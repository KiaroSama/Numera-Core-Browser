use core_tooling::chromium_build::{process, report};
use core_tooling::sha256_file;
use serde_json::{Value, json};
use std::env;
use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn pin(path: &Path) -> Result<Value> {
    if !path.is_absolute() || !path.symlink_metadata()?.is_file() {
        return Err("SDK inputs require absolute regular files".into());
    }
    Ok(json!({"path": path, "sha256": sha256_file(path)?}))
}

fn find_node() -> Result<PathBuf> {
    let search = env::var_os("PATH").ok_or("PATH is required to locate Node")?;
    let executable = format!("node{}", env::consts::EXE_SUFFIX);
    let found = env::split_paths(&search)
        .map(|dir| dir.join(&executable))
        .find(|path| path.is_file())
        .ok_or("The pinned external Node runtime was not found")?;
    Ok(found.canonicalize()?)
}

fn run() -> Result<()> {
    let package = Path::new("tests/native-automation/package.json").canonicalize()?;
    let lock = package.with_file_name("package-lock.json");
    let metadata: Value = serde_json::from_slice(&fs::read(&package)?)?;
    let node = find_node()?;
    let mut version = Command::new(&node);
    version.arg("--version").env_remove("NODE_OPTIONS");
    let version = process::stdout(&mut version, Duration::from_secs(10), 1024)?;
    let version = std::str::from_utf8(&version)?
        .trim()
        .trim_start_matches('v');
    if metadata["engines"]["node"] != version {
        return Err("External Node version differs from the fixture pin".into());
    }
    let program = process::binary_sibling("numera-sdk-fixture")?.canonicalize()?;
    let library = if cfg!(windows) {
        "core_sdk_fixture.dll"
    } else {
        "libcore_sdk_fixture.so"
    };
    let library = env::current_exe()?.with_file_name(library);
    let root = env::current_dir()?.join("build/native-sdk");
    fs::create_dir_all(&root)?;
    let module = root.join("core-sdk-fixture.node");
    fs::copy(library, &module)?;
    let specification = root.join("adapter.json");
    report::write_json(
        &specification,
        &json!({
            "schema_revision": 1,
            "program": pin(&program)?,
            "arguments": [&node, &module, &package],
            "files": [pin(&node)?, pin(&module)?, pin(&package)?, pin(&lock)?],
        }),
    )?;
    if let Some(github_environment) = env::var_os("GITHUB_ENV") {
        let path = specification
            .to_str()
            .ok_or("Adapter specification path must be UTF-8")?;
        if path.contains(['\r', '\n']) {
            return Err("Adapter specification path cannot contain a newline".into());
        }
        writeln!(
            OpenOptions::new().append(true).open(github_environment)?,
            "NUMERA_SDK_ADAPTER={path}"
        )?;
    }
    println!("{}", specification.display());
    Ok(())
}

fn main() -> ExitCode {
    if let Err(error) = run() {
        report::log("ERROR", "native-sdk-setup", &error.to_string());
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
