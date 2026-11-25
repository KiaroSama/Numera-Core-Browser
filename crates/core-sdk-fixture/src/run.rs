use crate::node::{Context, Js, Ref, Result};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    env,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    rc::Rc,
};

pub const NODE: &str = "24.22.3";
pub const MARKER: &str = "NUMERA_RENDERER_FIXTURE_PASS";
pub const FRAME_MARKER: &str = "NUMERA_FRAME_FIXTURE_PASS";
pub const INPUT: &str = "Numera مسیر 2026";
pub const CHECKS: [&str; 5] = [
    "navigation",
    "unicode-input",
    "click-and-wait",
    "nested-frame",
    "png-screenshot",
];
pub const TIMEOUT: u64 = 15_000;
const PACKAGES: [(&str, &str); 3] = [
    ("selenium-webdriver", "4.53.0"),
    ("puppeteer-core", "25.18.1"),
    ("playwright-core", "1.65.1"),
];

type Step = Box<dyn FnOnce(&mut Run, Context, Js) -> Result<Js>>;

pub struct Arguments {
    pub driver: String,
    pub browser: String,
    pub profile: String,
    pub fixture: String,
    pub responsive: String,
    pub report: PathBuf,
}

pub struct Run {
    pub args: Arguments,
    pub browser_version: String,
    pub browser_endpoint: String,
    pub desktop: Value,
    pub layouts: Vec<Value>,
    references: BTreeMap<&'static str, Ref>,
    steps: VecDeque<Step>,
}

fn json_file(path: &Path) -> Result<Value> {
    const LIMIT: u64 = 1024 * 1024;
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|error| error.to_string())?
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > LIMIT {
        return Err("Approved SDK manifest exceeded its size limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

pub fn loopback(address: &str) -> Result<String> {
    let invalid = || "Expected a local Chromium automation endpoint".to_owned();
    let port = address
        .strip_prefix("localhost:")
        .or_else(|| address.strip_prefix("127.0.0.1:"))
        .ok_or_else(invalid)?;
    if port.is_empty()
        || port.len() > 5
        || port.starts_with('0')
        || !port.bytes().all(|byte| byte.is_ascii_digit())
        || port.parse::<u16>().is_err()
    {
        return Err(invalid());
    }
    Ok(format!("http://127.0.0.1:{port}"))
}

impl Arguments {
    fn read(ctx: Context) -> Result<Self> {
        let argv = ctx.get(ctx.process()?, "argv")?;
        if ctx.array_len(argv)? != 6 {
            return Err("Expected five native SDK fixture arguments".into());
        }
        let read = |index| ctx.text(ctx.element(argv, index)?);
        let driver = loopback(&read(1)?)?;
        let browser = read(2)?;
        let profile = read(3)?;
        let fixture = read(4)?;
        let report = PathBuf::from(read(5)?);
        if ![Path::new(&browser), Path::new(&profile), &report]
            .iter()
            .all(|path| path.is_absolute())
        {
            return Err("SDK fixture paths must be absolute".into());
        }
        let url = url::Url::parse(&fixture).map_err(|error| error.to_string())?;
        if url.scheme() != "file" || url.host_str().is_some_and(|host| host != "localhost") {
            return Err("Expected a local file URL for the Chromium fixture".into());
        }
        let responsive = url
            .join("responsive-viewport.html")
            .map_err(|error| error.to_string())?
            .into();
        Ok(Self {
            driver,
            browser,
            profile,
            fixture,
            responsive,
            report,
        })
    }
}

impl Run {
    pub fn stage(&mut self, step: impl FnOnce(&mut Self, Context, Js) -> Result<Js> + 'static) {
        self.steps.push_back(Box::new(step));
    }

    pub fn save(&mut self, ctx: Context, name: &'static str, object: Js) -> Result<()> {
        if let Some(previous) = self.references.insert(name, ctx.reference(object)?) {
            ctx.release(previous);
        }
        Ok(())
    }

    pub fn object(&self, ctx: Context, name: &str) -> Result<Js> {
        self.references
            .get(name)
            .copied()
            .ok_or_else(|| format!("SDK object is missing: {name}"))
            .and_then(|reference| ctx.dereference(reference))
    }

    pub fn act(&mut self, name: &'static str, method: &'static str) {
        self.stage(move |state, ctx, _| ctx.call(state.object(ctx, name)?, method, &[]));
    }

    pub fn keep(&mut self, name: &'static str) {
        self.stage(move |state, ctx, previous| {
            state.save(ctx, name, previous)?;
            ctx.undefined()
        });
    }

    fn release(&mut self, ctx: Context) {
        for reference in std::mem::take(&mut self.references).into_values() {
            ctx.release(reference);
        }
        self.steps.clear();
    }
}

fn load_packages(ctx: Context, run: &mut Run) -> Result<()> {
    let anchor = env::var("NUMERA_SDK_PACKAGE").map_err(|_| "Missing SDK package manifest path")?;
    let manifest = Path::new(&anchor);
    if !manifest.is_absolute()
        || manifest.file_name().and_then(|name| name.to_str()) != Some("package.json")
    {
        return Err("Expected an absolute package.json path for the approved SDKs".into());
    }
    let versions = ctx.get(ctx.process()?, "versions")?;
    let package_info = json_file(manifest)?;
    if package_info["engines"]["node"] != NODE || ctx.text(ctx.get(versions, "node")?)? != NODE {
        return Err("Node runtime does not match the approved SDK fixture engine".into());
    }
    let modules = manifest
        .parent()
        .ok_or("SDK manifest lacks its module directory")?
        .join("node_modules");
    let require = ctx.require(&anchor)?;
    let receiver = ctx.undefined()?;
    for (package, version) in PACKAGES {
        let directory = modules.join(package);
        if package_info["dependencies"][package] != version
            || json_file(&directory.join("package.json"))?["version"] != version
        {
            return Err(format!(
                "Installed SDK version does not match its pin: {package}"
            ));
        }
        let object = load_package(ctx, receiver, require, package, &directory)?;
        run.save(ctx, package, object)?;
    }
    let chrome = load_package(
        ctx,
        receiver,
        require,
        "selenium-webdriver/chrome.js",
        &modules.join("selenium-webdriver"),
    )?;
    run.save(ctx, "chrome", chrome)
}

fn load_package(
    ctx: Context,
    receiver: Js,
    require: Js,
    request: &str,
    expected: &Path,
) -> Result<Js> {
    let request = ctx.string(request)?;
    let resolved = ctx.text(ctx.call(require, "resolve", &[request])?)?;
    let path = fs::canonicalize(resolved).map_err(|error| error.to_string())?;
    let expected = fs::canonicalize(expected).map_err(|error| error.to_string())?;
    if !path.is_file() || !path.starts_with(expected) {
        return Err("SDK module resolved outside its approved package directory".into());
    }
    ctx.invoke(receiver, require, &[request])
}

pub fn expect_text(ctx: Context, observed: Js, expected: &str) -> Result<Js> {
    if ctx.text(observed)? != expected {
        return Err("Unexpected browser automation fixture content".into());
    }
    ctx.undefined()
}

pub fn write_report(state: &Run) -> Result<()> {
    let report = json!({
        "result":"PASS", "node": NODE, "browserVersion": state.browser_version,
        "tests": {
            "selenium":{"version":"4.53.0", "transport":"WebDriver", "checks":CHECKS},
            "puppeteer":{"version":"25.18.1", "transport":"Chrome DevTools Protocol", "checks":CHECKS},
            "playwright":{"version":"1.65.1", "transport":"connectOverCDP", "checks":CHECKS}
        },
        "responsiveViewport": {
            "result":"PASS", "layouts":state.layouts, "reload":"PASS", "separate_target":"PASS",
            "scope":"Chromium CDP viewport, screen, pixel ratio and touch on a controlled local document"
        },
        "scope":"Controlled headless navigation, Unicode input, frame, click and screenshot for each client"
    });
    if state.browser_version.is_empty() || state.layouts.len() != 2 {
        return Err("SDK fixture did not reach its full browser checks".into());
    }
    let mut file = File::create_new(&state.args.report).map_err(|error| error.to_string())?;
    serde_json::to_writer_pretty(&mut file, &report).map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())
}

pub fn start(ctx: Context) -> Result<()> {
    let mut state = Run {
        args: Arguments::read(ctx)?,
        browser_version: String::new(),
        browser_endpoint: String::new(),
        desktop: Value::Null,
        layouts: Vec::new(),
        references: BTreeMap::new(),
        steps: VecDeque::new(),
    };
    if let Err(error) = load_packages(ctx, &mut state) {
        state.release(ctx);
        return Err(error);
    }
    crate::scenarios::compose(&mut state);
    advance(Rc::new(RefCell::new(state)), ctx, ctx.undefined()?);
    Ok(())
}

fn advance(state: Rc<RefCell<Run>>, ctx: Context, previous: Js) {
    let step = state.borrow_mut().steps.pop_front();
    let Some(step) = step else {
        state.borrow_mut().release(ctx);
        return;
    };
    let output = step(&mut state.borrow_mut(), ctx, previous);
    match output {
        Ok(output) => {
            let success = Rc::clone(&state);
            let failure = Rc::clone(&state);
            if let Err(error) = ctx.then(
                output,
                Box::new(move |ctx, value| advance(success, ctx, value)),
                Box::new(move |ctx, rejected| abort(failure, ctx, ctx.error_text(rejected))),
            ) {
                abort(state, ctx, error);
            }
        }
        Err(error) => abort(state, ctx, error),
    }
}

fn abort(state: Rc<RefCell<Run>>, ctx: Context, error: String) {
    eprintln!("{error}");
    ctx.exit_code();
    let mut state = state.borrow_mut();
    if let Ok(browser) = state.object(ctx, "puppeteer_browser") {
        let _ = ctx.call(browser, "disconnect", &[]);
    }
    if let Ok(driver) = state.object(ctx, "driver")
        && let Ok(close) = ctx.call(driver, "quit", &[])
    {
        let _ = ctx.then(close, Box::new(|_, _| {}), Box::new(|_, _| {}));
    }
    state.release(ctx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_a_literal_loopback_endpoint() {
        assert_eq!(loopback("localhost:4444").unwrap(), "http://127.0.0.1:4444");
        assert_eq!(
            loopback("127.0.0.1:65535").unwrap(),
            "http://127.0.0.1:65535"
        );
        for rejected in [
            "example.invalid:4444",
            "127.0.0.1:0",
            "localhost:0444",
            "localhost:65536",
            "localhost:4444/path",
            "http://localhost:4444",
        ] {
            assert!(loopback(rejected).is_err(), "{rejected}");
        }
    }
}
