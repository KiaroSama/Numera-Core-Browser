use super::{Environment, Result, webdriver::fixture_url};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

mod processes;
#[cfg(windows)]
pub(super) use processes::windows_arguments;
pub(super) use processes::{descendants, exited, stop_tree};

pub(super) fn failure(text: impl Into<String>) -> Box<dyn std::error::Error> {
    io::Error::other(text.into()).into()
}

pub(super) fn utf8(value: &OsStr) -> Result<&str> {
    value
        .to_str()
        .ok_or_else(|| failure("Expected UTF-8 browser fixture input"))
}

pub(super) fn path_text(path: &Path) -> Result<&str> {
    utf8(path.as_os_str())
}

pub(super) fn command(program: impl AsRef<OsStr>, environment: &Environment) -> Command {
    let mut command = Command::new(program);
    command.env_clear().envs(environment);
    command
}

pub(super) fn sha256(path: &Path) -> Result<String> {
    Ok(crate::sha256_file(path)?)
}

pub(super) fn read_bounded(path: &Path, max: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(failure("Browser fixture file exceeds its size limit"));
    }
    Ok(bytes)
}

pub(super) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    Ok(serde_json::from_slice(&read_bounded(
        path,
        16 * 1024 * 1024,
    )?)?)
}

pub(super) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| failure("Missing evidence parent directory"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temporary, value)?;
    temporary.write_all(b"\n")?;
    temporary.persist(path)?;
    Ok(())
}

pub(super) fn tail(path: &Path, max: u64) -> Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    file.seek(SeekFrom::Start(size.saturating_sub(max)))?;
    let mut data = Vec::new();
    file.take(max).read_to_end(&mut data)?;
    Ok(data)
}

pub(super) fn wait(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        sleep(Duration::from_millis(25));
    }
}

pub fn timed(command: &mut Command, seconds: u64) -> Result<ExitStatus> {
    let mut child = command.spawn()?;
    match wait(&mut child, Duration::from_secs(seconds)) {
        Ok(Some(status)) => Ok(status),
        outcome => {
            stop_tree(&mut child);
            outcome?.ok_or_else(|| failure("Browser fixture process timed out"))
        }
    }
}

pub(super) fn capture(
    command: &mut Command,
    seconds: u64,
    limit: u64,
    combine_logs: bool,
) -> Result<(ExitStatus, Vec<u8>)> {
    let mut stdout = tempfile::tempfile()?;
    command.stdout(Stdio::from(stdout.try_clone()?));
    if combine_logs {
        command.stderr(Stdio::from(stdout.try_clone()?));
    }
    let status = timed(command, seconds)?;
    stdout.rewind()?;
    let mut bytes = Vec::new();
    stdout.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(failure(
            "Browser fixture process output exceeds its size limit",
        ));
    }
    Ok((status, bytes))
}

pub(super) fn capture_success(command: &mut Command, seconds: u64, limit: u64) -> Result<Vec<u8>> {
    let (status, bytes) = capture(command, seconds, limit, false)?;
    if !status.success() {
        return Err(failure("Browser fixture process exited with a failure"));
    }
    Ok(bytes)
}

pub(super) fn with_prefix(prefix: &str, path: &Path) -> OsString {
    let mut value = OsString::from(prefix);
    value.push(path);
    value
}

pub(super) fn absolute(path: &Path) -> Result<PathBuf> {
    Ok(std::path::absolute(path)?)
}

pub fn smoke_renderer(
    binary: &Path,
    scratch: &Path,
    output: &Path,
    environment: &Environment,
) -> Result<()> {
    let binary = absolute(binary)?;
    let browser_parent = binary
        .parent()
        .ok_or_else(|| failure("Missing browser binary directory"))?;
    let profile = absolute(&scratch.join("fixture-profile"))?;
    let screenshot = absolute(&output.join("renderer.png"))?;
    let fixture = OsString::from(fixture_url()?);
    let headless = [
        "--headless=new".into(),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        with_prefix("--user-data-dir=", &profile),
    ];
    for name in ["version", "renderer", "screenshot"] {
        let mut command = command(&binary, environment);
        command
            .current_dir(browser_parent)
            .stdout(File::create(output.join(format!("smoke-{name}.stdout")))?)
            .stderr(File::create(output.join(format!("smoke-{name}.stderr")))?);
        match name {
            "version" => {
                command.arg("--version");
            }
            "renderer" => {
                command.args(&headless).arg("--dump-dom").arg(&fixture);
            }
            _ => {
                command
                    .args(&headless)
                    .arg("--window-size=800,600")
                    .arg(with_prefix("--screenshot=", &screenshot))
                    .arg(&fixture);
            }
        }
        if !timed(&mut command, 60)?.success() {
            return Err(failure(
                "Controlled source-built browser exited with a failure",
            ));
        }
    }
    let html = read_bounded(&output.join("smoke-renderer.stdout"), 1024 * 1024)?;
    let html = String::from_utf8_lossy(&html);
    if !html.contains("<strong id=\"result\">NUMERA_RENDERER_FIXTURE_PASS</strong>") {
        return Err(failure("Renderer did not execute the fixture script"));
    }
    let mut signature = [0; 8];
    if File::open(screenshot)?.read_exact(&mut signature).is_err()
        || signature != *b"\x89PNG\r\n\x1a\n"
    {
        return Err(failure("Screenshot fixture did not produce a PNG"));
    }
    Ok(())
}

#[cfg(unix)]
pub fn install_linux_sandbox(
    native_output: &Path,
    scratch: &Path,
    environment: &mut Environment,
) -> Result<Value> {
    use serde_json::json;
    use std::{fs, os::unix::fs::MetadataExt};
    let source = native_output.join("chrome_sandbox");
    let helper = scratch.join("numera-source-sandbox");
    if !fs::symlink_metadata(&source)?.is_file() || helper.symlink_metadata().is_ok() {
        return Err(failure(
            "Owned sandbox helper source/destination check failed",
        ));
    }
    let status = timed(
        command("sudo", environment)
            .args([
                "-n", "--", "install", "-o", "root", "-g", "root", "-m", "4755", "--",
            ])
            .arg(&source)
            .arg(&helper),
        30,
    )?;
    let metadata = fs::symlink_metadata(&helper)?;
    let digest = sha256(&helper)?;
    if !status.success()
        || !metadata.is_file()
        || metadata.uid() != 0
        || metadata.mode() & 0o7777 != 0o4755
        || digest != sha256(&source)?
    {
        return Err(failure(
            "Owned sandbox helper installation verification failed",
        ));
    }
    environment.insert("CHROME_DEVEL_SANDBOX".into(), helper.into_os_string());
    Ok(
        json!({"source_target":"chrome_sandbox","sha256":digest,"owner_uid":0,"mode":"4755",
              "scope":"Owned-source SUID developer fixture; no global AppArmor/sysctl or browser sandbox disabling"}),
    )
}

#[cfg(not(unix))]
pub fn install_linux_sandbox(_: &Path, _: &Path, _: &mut Environment) -> Result<Value> {
    Err(failure("Linux sandbox installation requires native Linux"))
}
