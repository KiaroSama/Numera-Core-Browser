//! Defensive driver for the pinned scanner. Repository contents are never executed.
#[path = "numera-gitleaks/config.rs"]
mod config;
#[path = "numera-gitleaks/controls.rs"]
mod controls;
#[path = "numera-gitleaks/process.rs"]
mod process;
#[path = "numera-gitleaks/summary.rs"]
mod summary;
#[cfg(test)]
#[path = "numera-gitleaks/tests.rs"]
mod tests;

use serde_json::Value;
use std::{
    env,
    ffi::OsString,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

type Result<T> = std::result::Result<T, String>;
const VERSION: &str = "8.30.1";
const ARCHIVE_SHA256: &str = "551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb";
const DOWNLOAD_URL: &str = "https://github.com/gitleaks/gitleaks/releases/download/v8.30.1/gitleaks_8.30.1_linux_x64.tar.gz";

fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(message.to_owned())
    }
}

fn log(level: &str, message: &str) {
    println!("{} [{level}] {message}", jiff::Timestamp::now());
}

fn environment(name: &str) -> Result<OsString> {
    env::var_os(name)
        .ok_or_else(|| "A required scanner environment variable is unavailable.".to_owned())
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut options = File::options();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = options
        .open(path)
        .map_err(|_| "A scanner evidence file could not be read.")?;
    require(
        file.metadata()
            .map_err(|_| "A scanner evidence file could not be inspected.")?
            .is_file(),
        "Scanner evidence must be a regular file.",
    )?;
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "A scanner evidence file could not be read.".to_owned())?;
    require(
        bytes.len() as u64 <= limit,
        "A scanner evidence file exceeded its size limit.",
    )?;
    Ok(bytes)
}

fn read_json(path: &Path) -> Result<Value> {
    serde_json::from_slice(&read_bounded(path, 64 * 1024 * 1024)?)
        .map_err(|_| "A scanner evidence file is not valid JSON.".to_owned())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    File::options()
        .create_new(true)
        .write(true)
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
        .map_err(|_| "A private scanner file could not be created.".to_owned())
}

fn scanner(temporary: &Path, archive_override: Option<&Path>) -> Result<PathBuf> {
    let downloaded = temporary.join("gitleaks.tar.gz");
    let archive = match archive_override {
        Some(path) => path,
        None => {
            let mut command = process::command("curl");
            command
                .args([
                    "--disable",
                    "--fail",
                    "--silent",
                    "--show-error",
                    "--location",
                    "--proto",
                    "=https",
                    "--proto-redir",
                    "=https",
                    "--max-redirs",
                    "5",
                    "--connect-timeout",
                    "20",
                    "--max-time",
                    "60",
                    "--max-filesize",
                    "33554432",
                    "--output",
                ])
                .arg(&downloaded)
                .arg(DOWNLOAD_URL);
            let child = process::capture(&mut command, 70, 1024 * 1024)?;
            require(
                child.status.success(),
                "The pinned Gitleaks archive could not be downloaded.",
            )?;
            &downloaded
        }
    };
    let bytes = read_bounded(archive, 32 * 1024 * 1024)?;
    require(
        core_tooling::sha256_hex(&bytes) == ARCHIVE_SHA256,
        "The Gitleaks download checksum does not match the pinned release.",
    )?;
    let binary = temporary.join("gitleaks");
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()));
    let mut found = false;
    for member in archive
        .entries()
        .map_err(|_| "The verified scanner archive could not be read.")?
    {
        let mut member = member.map_err(|_| "The verified scanner archive could not be read.")?;
        let path = member
            .path()
            .map_err(|_| "The verified scanner archive has an invalid path.")?;
        if path.as_ref() != Path::new("gitleaks") {
            continue;
        }
        require(
            !found && member.header().entry_type().is_file() && member.size() <= 64 * 1024 * 1024,
            "The verified archive does not contain the expected executable.",
        )?;
        let mut data = Vec::new();
        member
            .read_to_end(&mut data)
            .map_err(|_| "The verified scanner executable could not be read.")?;
        write_new(&binary, &data)?;
        found = true;
    }
    require(
        found,
        "The verified archive does not contain the expected executable.",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))
            .map_err(|_| "The verified scanner executable could not be prepared.")?;
    }
    let result = process::capture(process::command(&binary).arg("version"), 15, 4096)?;
    require(
        result.status.success()
            && result.stdout.trim_ascii() == VERSION.as_bytes()
            && result.stderr.trim_ascii().is_empty(),
        "The Gitleaks executable version could not be verified.",
    )?;
    Ok(binary)
}

fn revisions(
    event_name: &str,
    event_path: Option<&Path>,
    git: &Path,
    git_directory: &Path,
) -> Result<(String, &'static str)> {
    let mut options =
        "--full-history --diff-filter=uxdb --no-ext-diff --no-textconv --ignore-submodules=all -m"
            .to_owned();
    if event_name == "pull_request" {
        let event = read_json(event_path.ok_or("The pull request event is unavailable.")?)?;
        let base = config::revision(&event["pull_request"]["base"]["sha"])?;
        let head = config::revision(&event["pull_request"]["head"]["sha"])?;
        for revision in [&base, &head] {
            let child = process::capture(
                process::command(git).arg("-C").arg(git_directory).args([
                    "cat-file",
                    "-e",
                    &format!("{revision}^{{commit}}"),
                ]),
                15,
                64 * 1024,
            )?;
            require(
                child.status.success(),
                "A pull request revision is missing from the full checkout.",
            )?;
        }
        options.push_str(&format!(" {base}..{head}"));
        Ok((
            options,
            "Commits introduced by this pull request, including merge diffs",
        ))
    } else {
        options.push_str(" --all");
        Ok((
            options,
            "All fetched branches and tags, including full history and merge diffs",
        ))
    }
}

fn scan(archive: Option<&Path>) -> Result<u8> {
    require(
        cfg!(all(target_os = "linux", target_arch = "x86_64")),
        "The pinned Gitleaks scanner requires Linux x64.",
    )?;
    // The workflow can build this trusted executable beside the untrusted checkout.
    let workspace = PathBuf::from(
        env::var_os("NUMERA_GITLEAKS_SCAN_ROOT")
            .map(Ok)
            .unwrap_or_else(|| environment("GITHUB_WORKSPACE"))?,
    )
    .canonicalize()
    .map_err(|_| "The complete Git checkout is unavailable.")?;
    let git_directory = workspace.join(".git");
    require(
        git_directory.is_dir(),
        "The complete Git checkout is unavailable.",
    )?;
    let event_name = environment("GITHUB_EVENT_NAME")?
        .into_string()
        .map_err(|_| "Unsupported workflow event.")?;
    require(
        ["push", "pull_request", "schedule", "workflow_dispatch"].contains(&event_name.as_str()),
        "Unsupported workflow event.",
    )?;
    log("INFO", "Starting the repository secret scan.");
    log(
        "DEBUG",
        "Using checksum-pinned Gitleaks 8.30.1; repository contents are not executed.",
    );
    let temporary = tempfile::Builder::new()
        .prefix("gitleaks-")
        .tempdir_in(environment("RUNNER_TEMP")?)
        .map_err(|_| "A private scanner directory could not be created.")?;
    let root = temporary.path();
    let binary = scanner(root, archive)?;
    log(
        "INFO",
        "Verified the scanner version and download checksum.",
    );
    let git = process::find_git()?;
    let config = root.join("scanner.toml");
    write_new(
        &config,
        config::scanner_config(&git, &git_directory)?.as_bytes(),
    )?;
    controls::verify(&binary, &git, &config, root)?;
    let event_path = env::var_os("GITHUB_EVENT_PATH").map(PathBuf::from);
    let (options, scope) = revisions(&event_name, event_path.as_deref(), &git, &git_directory)?;
    log("DEBUG", &format!("Scan scope: {scope}."));
    let status_path = root.join("git-status");
    let proxy = root.join("git");
    let copy = || -> std::io::Result<()> {
        // The held process image stays readable if another Cargo build replaces
        // the executable path while this scan is in progress.
        #[cfg(target_os = "linux")]
        let mut source = File::open("/proc/self/exe")?;
        #[cfg(not(target_os = "linux"))]
        let mut source = File::open(env::current_exe()?)?;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut destination = options.open(&proxy)?;
        let mut buffer = [0; 64 * 1024];
        loop {
            let count = source.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            destination.write_all(&buffer[..count])?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            destination.set_permissions(fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    };
    copy().map_err(|error| {
        format!(
            "The Git proxy executable could not be prepared ({:?}).",
            error.kind()
        )
    })?;
    let mut paths = vec![root.to_owned()];
    paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    let mut command = process::scan_command(
        &binary,
        &git_directory,
        &config,
        root,
        &root.join("findings.json"),
        &options,
    );
    command
        .env(
            "PATH",
            env::join_paths(paths).map_err(|_| "The private Git search path is invalid.")?,
        )
        .env("GITLEAKS_REAL_GIT", &git)
        .env("GITLEAKS_GIT_STATUS", &status_path);
    let result = process::capture(&mut command, 930, 1024 * 1024)?;
    let report = read_json(&root.join("findings.json"))?;
    let status = read_bounded(&status_path, 128)?;
    let findings = summary::validate_scan(&result, &status, report)?;
    summary::write_summary(&findings, scope)?;
    if findings.is_empty() {
        log(
            "INFO",
            "The scan completed successfully; no matching secrets were detected.",
        );
        Ok(0)
    } else {
        log(
            "WARNING",
            &format!(
                "Detected {} potential secrets. Review the redacted locations in the job summary.",
                findings.len()
            ),
        );
        Ok(1)
    }
}

fn run(args: &[OsString]) -> Result<u8> {
    require(
        env::var_os("RUNNER_DEBUG").as_deref() != Some(std::ffi::OsStr::new("1")),
        "Disable Actions step debug logging before scanning; checkout debug output can disclose commit contents.",
    )?;
    match args {
        [mode] if mode == "--privacy-guard" => {
            log("INFO", "Diagnostic privacy guard passed.");
            Ok(0)
        }
        [mode, value] if mode == "--safe-cell" => {
            println!(
                "{}",
                summary::safe_cell(value.to_str().ok_or("Expected UTF-8 summary input.")?)
            );
            Ok(0)
        }
        [mode, archive] if mode == "--verify-controls" => {
            require(
                cfg!(all(target_os = "linux", target_arch = "x86_64")),
                "The pinned Gitleaks scanner requires Linux x64.",
            )?;
            let temporary = tempfile::tempdir()
                .map_err(|_| "A private scanner directory could not be created.")?;
            let binary = scanner(temporary.path(), Some(Path::new(archive)))?;
            let config = temporary.path().join("scanner.toml");
            write_new(&config, config::render_config().as_bytes())?;
            controls::verify(&binary, &process::find_git()?, &config, temporary.path())?;
            Ok(0)
        }
        [] => scan(None),
        [mode, archive] if mode == "--archive" => scan(Some(Path::new(archive))),
        _ => Err("Invalid Gitleaks driver arguments.".to_owned()),
    }
}

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().collect();
    if args
        .first()
        .and_then(|path| Path::new(path).file_name())
        .is_some_and(|name| name == "git" || name == "git.exe")
    {
        return ExitCode::from(process::git_proxy(&args[1..]));
    }
    if args.get(1).is_some_and(|mode| mode == "--git-proxy") {
        return ExitCode::from(process::git_proxy(&args[2..]));
    }
    match run(&args[1..]) {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            log("ERROR", &message);
            ExitCode::FAILURE
        }
    }
}
