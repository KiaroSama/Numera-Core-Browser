#[path = "numera-automation-baseline/contract.rs"]
mod contract;
#[path = "numera-automation-baseline/hosted.rs"]
mod hosted;
#[path = "numera-automation-baseline/sdk.rs"]
mod sdk;

use core_tooling::fixtures::{self, Environment};
use serde_json::{Value, json};
use std::{
    env,
    error::Error,
    ffi::OsString,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn failure(text: &str) -> Box<dyn Error> {
    std::io::Error::other(text).into()
}

pub(crate) fn digest(path: &Path) -> Result<String> {
    Ok(core_tooling::sha256_file(path)?)
}

pub(crate) fn read_json(path: &Path) -> Result<Value> {
    let mut data = Vec::new();
    File::open(path)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut data)?;
    if data.len() > 1024 * 1024 {
        return Err(failure("Automation JSON exceeds its size limit"));
    }
    Ok(serde_json::from_slice(&data)?)
}

fn write_report(path: &Path, report: &Value) -> Result<()> {
    let mut file = File::options().write(true).create_new(true).open(path)?;
    serde_json::to_writer_pretty(&mut file, report)?;
    file.write_all(b"\n")?;
    Ok(())
}

#[derive(Default)]
struct Options {
    browser: Option<PathBuf>,
    driver: Option<PathBuf>,
    output: Option<PathBuf>,
    scratch: Option<PathBuf>,
    adapter: Option<PathBuf>,
    local: bool,
}

fn options(args: &[OsString]) -> Result<Options> {
    let mut output = Options::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--local" {
            if output.local {
                return Err(failure("Duplicate --local argument"));
            }
            output.local = true;
            continue;
        }
        let field = match arg.to_str() {
            Some("--browser") => &mut output.browser,
            Some("--driver") => &mut output.driver,
            Some("--output") => &mut output.output,
            Some("--scratch") => &mut output.scratch,
            Some("--sdk-adapter") => &mut output.adapter,
            _ => return Err(failure("Unknown automation baseline argument")),
        };
        if field.is_some() {
            return Err(failure("Duplicate automation baseline argument"));
        }
        *field = Some(PathBuf::from(args.next().ok_or_else(|| {
            failure("Missing automation baseline argument value")
        })?));
    }
    if output.local && (output.browser.is_none() || output.driver.is_none()) {
        return Err(failure(
            "Local baseline requires explicit --browser and --driver paths",
        ));
    }
    if !output.local && (output.browser.is_some() || output.driver.is_some()) {
        return Err(failure("Browser overrides require explicit --local mode"));
    }
    Ok(output)
}

fn hosted_guard(environment: &Environment) -> Result<()> {
    let value = |name: &str| hosted::value(environment, name).and_then(|v| v.to_str());
    let event_path =
        value("GITHUB_EVENT_PATH").ok_or_else(|| failure("Missing hosted event path"))?;
    let event = read_json(Path::new(event_path))?;
    if value("GITHUB_ACTIONS") != Some("true")
        || value("RUNNER_ENVIRONMENT") != Some("github-hosted")
        || value("GITHUB_REPOSITORY") != Some("KiaroSama/Numera-Core-Browser")
        || event["repository"]["private"] != false
        || value("RUNNER_ARCH") != Some("X64")
        || value("GITHUB_SHA").is_none()
        || value("ImageVersion").is_none()
        || !cfg!(any(target_os = "windows", target_os = "linux"))
    {
        return Err(failure(
            "The comparison baseline requires an authorized hosted Windows or Linux job",
        ));
    }
    Ok(())
}

fn find_browser(environment: &Environment) -> Result<PathBuf> {
    if cfg!(windows) {
        for name in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(root) = hosted::value(environment, name) {
                let candidate = Path::new(root).join("Google/Chrome/Application/chrome.exe");
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    } else if let Some(path) = hosted::value(environment, "PATH") {
        for root in env::split_paths(path) {
            let candidate = root.join("google-chrome");
            if executable_file(&candidate) {
                return Ok(candidate);
            }
        }
    }
    Err(failure("The hosted image did not supply Google Chrome"))
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}

fn baseline(options: Options) -> Result<Value> {
    if !cfg!(any(target_os = "windows", target_os = "linux")) {
        return Err(failure("The comparison baseline requires Windows or Linux"));
    }
    let environment: Environment = env::vars_os().collect();
    if !options.local {
        hosted_guard(&environment)?;
    }
    let output = std::path::absolute(
        options
            .output
            .unwrap_or_else(|| PathBuf::from("build/automation-baseline")),
    )?;
    let scratch = std::path::absolute(options.scratch.unwrap_or_else(|| {
        hosted::value(&environment, "RUNNER_TEMP")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir)
            .join("Numera fixture baseline")
    }))?;
    if output.exists()
        || scratch.exists()
        || output.starts_with(&scratch)
        || scratch.starts_with(&output)
    {
        return Err(failure(
            "Baseline output and scratch must be new separate directories",
        ));
    }
    fs::create_dir_all(&output)?;
    fs::create_dir_all(&scratch)?;
    let mut report = json!({"schema_revision":1,"source_commit":env::var("GITHUB_SHA").ok(),
        "runner_image":env::var("ImageVersion").ok(),"host":if cfg!(windows) {"Windows"} else {"Linux"},
        "baseline":if options.local {"explicit local stock browser"} else {"runner-installed Google Chrome"},
        "owned_engine_used":false,"source_build":"NOT_RUN","result":"BLOCKED"});
    let environment = contract::child_environment(environment);
    let outcome = (|| -> Result<()> {
        let browser = options
            .browser
            .map_or_else(|| find_browser(&environment), Ok)?
            .canonicalize()?;
        let driver = match options.driver {
            Some(path) => path,
            None => hosted::driver(&environment)?,
        }
        .canonicalize()?;
        if !browser.is_file() || !driver.is_file() {
            return Err(failure("Baseline browser and driver must be regular files"));
        }
        report["browser_sha256"] = json!(digest(&browser)?);
        report["chromedriver_sha256"] = json!(digest(&driver)?);
        fixtures::smoke_renderer(&browser, &scratch, &output, &environment)?;
        report["renderer_fixture"] = json!("PASS");
        report["webdriver"] =
            fixtures::smoke_driver(&browser, &driver, &scratch, &output, &environment, false)?;
        report["automation_clients"] = sdk::smoke_clients(
            &browser,
            &driver,
            &scratch,
            &output,
            &environment,
            options.adapter.as_deref(),
        )?;
        report["headed_browser"] =
            fixtures::smoke_headed_browser(&browser, &driver, &scratch, &output, &environment)?;
        report["chromium_ownership"] =
            fixtures::smoke_browser_ownership(&browser, &scratch, &output, &environment)?;
        report["profile_data"] =
            fixtures::smoke_profile_data(&browser, &driver, &scratch, &output, &environment)?;
        report["result"] = json!(if report["automation_clients"]["result"] == "PASS" {
            "PASS"
        } else {
            "PARTIAL"
        });
        Ok(())
    })();
    report["timestamp"] = json!(jiff::Timestamp::now().to_string());
    if outcome.is_err() {
        report["failure"] = json!("A baseline fixture failed; inspect retained fixture logs");
    }
    write_report(&output.join("report.json"), &report)?;
    outcome?;
    Ok(report)
}

fn run() -> Result<Value> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.as_slice() == [OsString::from("--help")] {
        return Ok(
            json!({"usage":"numera-automation-baseline [--sdk-adapter SPEC] [--output DIR] [--scratch DIR] [--local --browser FILE --driver FILE]",
            "sdk_clients":"--sdk-clients BROWSER DRIVER SCRATCH OUTPUT [--sdk-adapter SPEC]",
            "adapter_schema":{"schema_revision":1,"program":{"path":"absolute executable","sha256":"lowercase SHA-256"},"arguments":["absolute pinned runner file"],"files":[{"path":"absolute runner file","sha256":"lowercase SHA-256"}]},
            "scope":"Stock browser harness comparison; no owned engine or source build acceptance",
            "missing_adapter":"SDK scenarios NOT_RUN; overall baseline PARTIAL"}),
        );
    }
    if args.first().is_some_and(|mode| mode == "--sdk-clients") {
        let (paths, adapter) = match &args[1..] {
            [browser, driver, scratch, output] => ([browser, driver, scratch, output], None),
            [browser, driver, scratch, output, flag, adapter] if flag == "--sdk-adapter" => {
                ([browser, driver, scratch, output], Some(Path::new(adapter)))
            }
            _ => {
                return Err(failure(
                    "Expected --sdk-clients BROWSER DRIVER SCRATCH OUTPUT [--sdk-adapter SPEC]",
                ));
            }
        };
        let environment = contract::child_environment(env::vars_os().collect());
        return sdk::smoke_clients(
            &Path::new(paths[0]).canonicalize()?,
            &Path::new(paths[1]).canonicalize()?,
            &Path::new(paths[2]).canonicalize()?,
            &Path::new(paths[3]).canonicalize()?,
            &environment,
            adapter,
        );
    }
    if let [mode, input] = args.as_slice()
        && mode == "--loopback"
    {
        let value: Value = serde_json::from_str(
            input
                .to_str()
                .ok_or_else(|| failure("Expected UTF-8 JSON"))?,
        )?;
        return Ok(match contract::loopback_port(&value) {
            Ok(url) => json!({"ok":true,"value":url}),
            Err(error) => json!({"ok":false,"error":error}),
        });
    }
    baseline(options(&args)?)
}

fn main() -> ExitCode {
    match run() {
        Ok(value) => {
            if let Some(ok) = value.get("ok").and_then(Value::as_bool) {
                // Preserve the characterization wrapper's field order, including errors.
                let field = if ok { "value" } else { "error" };
                println!("{{\"ok\":{ok},\"{field}\":{}}}", value[field]);
            } else {
                println!("{value}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}
