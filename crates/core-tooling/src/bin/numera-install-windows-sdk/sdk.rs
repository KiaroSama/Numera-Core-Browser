use crate::trust;
use core_tooling::chromium_build::process::ChildWatch;
use core_tooling::sha256_file;
use serde::Deserialize;
use serde_json::{Value, json};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Deserialize)]
struct SdkLock {
    release: String,
    sdk_directory_version: String,
    resolved_url: String,
    sha256: String,
    bytes: u64,
}

fn stage(root: &Path, phase: &str) -> Result<()> {
    fs::write(root.join("phase.txt"), phase)?;
    let event = json!({"timestamp": jiff::Timestamp::now().to_string(), "phase": phase});
    let mut history = OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("phases.ndjson"))?;
    writeln!(history, "{event}")?;
    eprintln!("{event}");
    Ok(())
}

fn complete(kit: &Path, version: &str) -> Option<trust::DebuggerVersion> {
    let include = kit.join("Include").join(version);
    if ["um", "shared", "ucrt", "winrt", "cppwinrt"]
        .into_iter()
        .any(|dir| !include.join(dir).is_dir())
    {
        return None;
    }
    let lib = kit.join("Lib").join(version);
    if ["um/x86", "um/x64", "ucrt/x86", "ucrt/x64"]
        .into_iter()
        .any(|dir| !lib.join(dir).is_dir())
        || !kit.join("bin").join(version).join("x64/rc.exe").is_file()
    {
        return None;
    }
    let version = trust::debugger_version(&kit.join("Debuggers/x64/dbghelp.dll")).ok()?;
    ((version.parts[2], version.parts[3]) >= (26100, 3323)).then_some(version)
}

fn install(root: &Path, lock: &SdkLock, report: &mut Value) -> Result<()> {
    let installer = root.join("winsdksetup.exe");
    stage(root, "windows-sdk-download")?;
    let url = url::Url::parse(&lock.resolved_url)?;
    if url.scheme() != "https"
        || url.host_str() != Some("download.microsoft.com")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Unexpected SDK installer source".into());
    }
    let status = Command::new("curl.exe")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--proto",
            "=https",
            "--max-time",
            "120",
            "--max-filesize",
        ])
        .arg(lock.bytes.to_string())
        .arg("--output")
        .arg(&installer)
        .arg(url.as_str())
        .status()?;
    if !status.success()
        || fs::metadata(&installer)?.len() != lock.bytes
        || sha256_file(&installer)? != lock.sha256
    {
        return Err("SDK installer download or hash verification failed".into());
    }
    stage(root, "windows-sdk-publisher-verification")?;
    report["installer_signer"] = json!(trust::publisher(&installer)?);
    stage(root, "windows-sdk-installation")?;
    let status = ChildWatch::spawn(
        Command::new(&installer)
            .args(["/features", "+", "/quiet", "/norestart", "/log"])
            .arg(root.join("windows-sdk.log")),
    )?
    .wait(Duration::from_secs(900))?;
    report["installer_exit_code"] = json!(status.code());
    if !matches!(status.code(), Some(0 | 3010)) {
        return Err("SDK prerequisite installation failed".into());
    }
    report["installed_by_task"] = json!(true);
    report["applied_installer_release"] = json!(lock.release);
    Ok(())
}

pub fn run() -> Result<()> {
    if [
        ("GITHUB_ACTIONS", "true"),
        ("RUNNER_ENVIRONMENT", "github-hosted"),
        ("GITHUB_REPOSITORY", "KiaroSama/Numera-Core-Browser"),
    ]
    .into_iter()
    .any(|(name, expected)| env::var(name).as_deref() != Ok(expected))
    {
        return Err("SDK preparation requires the authorized Windows GitHub-hosted runner".into());
    }
    let mut args = env::args_os().skip(1);
    let root = match (args.next(), args.next(), args.next()) {
        (Some(flag), Some(path), None) if flag == "--evidence-root" => PathBuf::from(path),
        _ => return Err("Usage: numera-install-windows-sdk --evidence-root DIRECTORY".into()),
    };
    let lock: SdkLock = serde_json::from_slice(&fs::read("upstream/windows-sdk/lock.json")?)?;
    let kit =
        PathBuf::from(env::var_os("ProgramFiles(x86)").ok_or("Windows SDK directory unavailable")?)
            .join("Windows Kits/10");
    let mut report = json!({
        "requested_release": lock.release, "directory_version": lock.sdk_directory_version,
        "installer_sha256": lock.sha256, "installed_by_task": false, "ready": false
    });
    let result = (|| {
        if complete(&kit, &lock.sdk_directory_version).is_none() {
            install(&root, &lock, &mut report)?;
        }
        stage(&root, "windows-sdk-validation")?;
        let debugger = complete(&kit, &lock.sdk_directory_version)
            .ok_or("Required complete SDK and debugging tools are still unavailable")?;
        report["debugging_tools_version"] = json!(debugger.file_version);
        report["ready"] = json!(true);
        Ok(())
    })();
    fs::write(
        root.join("windows-sdk.json"),
        format!("{}\n", serde_json::to_string_pretty(&report)?),
    )?;
    result
}
