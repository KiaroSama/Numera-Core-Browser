use super::{Result, environment, require};
use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
};

pub struct Captured {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command.env_clear().envs(env::vars_os().filter(|(key, _)| {
        ![
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "ACTIONS_RUNTIME_TOKEN",
            "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
            "ACTIONS_ID_TOKEN_REQUEST_URL",
        ]
        .iter()
        .any(|name| key.eq_ignore_ascii_case(OsStr::new(name)))
    }));
    command.stdin(Stdio::null());
    command
}

fn captured_bytes(file: &mut File, limit: u64) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))
        .map_err(|_| "Private scanner output could not be read.")?;
    let mut output = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut output)
        .map_err(|_| "Private scanner output could not be read.")?;
    require(
        output.len() as u64 <= limit,
        "Private scanner output exceeded its size limit; no clean result was recorded.",
    )?;
    Ok(output)
}

pub fn capture(command: &mut Command, seconds: u64, limit: u64) -> Result<Captured> {
    let mut stdout =
        tempfile::tempfile().map_err(|_| "Private scanner output could not be prepared.")?;
    let mut stderr =
        tempfile::tempfile().map_err(|_| "Private scanner output could not be prepared.")?;
    command
        .stdout(
            stdout
                .try_clone()
                .map_err(|_| "Private scanner output could not be prepared.")?,
        )
        .stderr(
            stderr
                .try_clone()
                .map_err(|_| "Private scanner output could not be prepared.")?,
        );
    let status = core_tooling::fixtures::bounded_child(command, seconds)
        .map_err(|_| "A bounded scanner child could not complete; no clean result was recorded.")?;
    Ok(Captured {
        status,
        stdout: captured_bytes(&mut stdout, limit)?,
        stderr: captured_bytes(&mut stderr, limit)?,
    })
}

pub fn find_git() -> Result<PathBuf> {
    for directory in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        let candidate = directory.join(if cfg!(windows) { "git.exe" } else { "git" });
        let Ok(metadata) = candidate.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 == 0 {
                continue;
            }
        }
        return candidate
            .canonicalize()
            .map_err(|_| "The Git executable is unavailable.".to_owned());
    }
    Err("The Git executable is unavailable.".to_owned())
}

pub fn scan_command(
    binary: &Path,
    git_directory: &Path,
    config: &Path,
    temporary: &Path,
    report: &Path,
    options: &str,
) -> Command {
    let mut child = command(binary);
    child
        .arg("git")
        .arg(git_directory)
        .arg("--config")
        .arg(config)
        .arg("--gitleaks-ignore-path")
        .arg(temporary)
        .args([
            "--ignore-gitleaks-allow",
            "--redact=100",
            "--no-banner",
            "--no-color",
            "--log-level=error",
            "--exit-code=2",
            "--report-format=json",
            "--report-path",
        ])
        .arg(report)
        .arg(format!("--log-opts={options}"));
    child
}

fn exit_number(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return -signal;
        }
    }
    1
}

fn proxy(args: &[OsString]) -> Result<u8> {
    let git = PathBuf::from(environment("GITLEAKS_REAL_GIT")?)
        .canonicalize()
        .map_err(|_| "The verified Git child could not complete.")?;
    require(
        git.is_absolute()
            && git.is_file()
            && git
                != env::current_exe().map_err(|_| "The verified Git child could not complete.")?,
        "The verified Git child could not complete.",
    )?;
    let history = args.get(2).is_some_and(|arg| arg == "log");
    let mut child = command(git);
    child
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let status = core_tooling::fixtures::bounded_child(&mut child, 920)
        .map_err(|_| "The verified Git child could not complete.")?;
    let number = exit_number(status);
    if history {
        let status_path = PathBuf::from(environment("GITLEAKS_GIT_STATUS")?);
        fs::write(status_path, number.to_string())
            .map_err(|_| "The verified Git child could not complete.")?;
        if number != 0 {
            let _ = writeln!(
                std::io::stderr(),
                "{} [ERROR] The Git-history child did not complete successfully.",
                jiff::Timestamp::now()
            );
        }
    }
    Ok(u8::try_from(number).unwrap_or(1))
}

pub fn git_proxy(args: &[OsString]) -> u8 {
    match proxy(args) {
        Ok(code) => code,
        Err(message) => {
            // Scanner stderr stays private. This diagnostic never includes Git output.
            let _ = writeln!(
                std::io::stderr(),
                "{} [ERROR] {message}",
                jiff::Timestamp::now()
            );
            1
        }
    }
}
