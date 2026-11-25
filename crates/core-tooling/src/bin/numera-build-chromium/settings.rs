use serde::Deserialize;
use std::{collections::BTreeMap, env, path::PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Prerequisites,
    Acquire,
    Generate,
    Build,
}

pub struct Options {
    pub root: PathBuf,
    pub phase: Phase,
    pub workers: u8,
    pub component: bool,
    pub debug: bool,
    pub driver: bool,
    pub probe: bool,
    pub wrapper: String,
    pub reviewed_lock: Option<PathBuf>,
}

#[derive(Deserialize)]
pub struct SourceLock {
    pub upstream_url: String,
    pub depot_tools_url: String,
    pub depot_tools_revision: String,
    pub platform_pins: BTreeMap<String, Pin>,
    pub patches: Vec<Patch>,
}

#[derive(Deserialize)]
pub struct Pin {
    pub revision: String,
}

#[derive(Deserialize)]
pub struct Patch {
    pub path: PathBuf,
    pub sha256: String,
}

impl Options {
    pub fn parse() -> Result<Self, Box<dyn std::error::Error>> {
        let mut args = env::args_os().skip(1);
        let mut options = Self {
            root: PathBuf::new(),
            phase: Phase::Acquire,
            workers: 2,
            component: false,
            debug: false,
            driver: false,
            probe: false,
            wrapper: "ccache".to_owned(),
            reviewed_lock: None,
        };
        while let Some(flag) = args.next() {
            let mut value = || {
                args.next()
                    .ok_or_else(|| format!("Missing value for {}", flag.to_string_lossy()))
            };
            match flag.to_str() {
                Some("--source-root") => options.root = PathBuf::from(value()?),
                Some("--phase") => {
                    options.phase = match value()?.to_string_lossy().to_ascii_lowercase().as_str() {
                        "prerequisites" => Phase::Prerequisites,
                        "acquire" => Phase::Acquire,
                        "generate" => Phase::Generate,
                        "build" => Phase::Build,
                        _ => return Err("Invalid build phase".into()),
                    }
                }
                Some("--parallelism") => {
                    options.workers = value()?.to_str().ok_or("Invalid parallelism")?.parse()?
                }
                Some("--component-build") => options.component = true,
                Some("--debug-build") => options.debug = true,
                Some("--build-driver") => options.driver = true,
                Some("--compiler-cache-probe") => options.probe = true,
                Some("--probe-cache-tool") => {
                    options.wrapper = value()?.into_string().map_err(|_| "Invalid wrapper name")?
                }
                Some("--reviewed-graph-lock") => {
                    options.reviewed_lock = Some(PathBuf::from(value()?))
                }
                _ => return Err(format!("Unknown argument {}", flag.to_string_lossy()).into()),
            }
        }
        if options.root.as_os_str().is_empty() {
            return Err(
                "Usage: numera-build-chromium --source-root ABSOLUTE_PATH [--phase PHASE]".into(),
            );
        }
        if !(1..=4).contains(&options.workers)
            || !matches!(options.wrapper.as_str(), "ccache" | "sccache")
        {
            return Err("Invalid parallelism or compiler wrapper".into());
        }
        if options.probe && options.phase != Phase::Generate {
            return Err("Compiler cache probe is restricted to graph generation".into());
        }
        Ok(options)
    }
}
