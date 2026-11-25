use core_tooling::chromium_build::{metadata::Host, process, report};
use serde_json::{Value, json};
use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, ExitCode, ExitStatus},
    time::{Duration, Instant},
};

const REQUIRED_FREE: u64 = 100_000_000_000;
const DISK_GUARD: u64 = 20_000_000_000;
const COLD_BUDGET: Duration = Duration::from_secs(300);
const DISK_BLOCK: &str = "Upstream minimum free disk is not available; source checkout and compilation were not attempted.";
const USAGE: &str = "Usage: numera-feasibility (--output PATH | --check PATH)";

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> io::Result<bool> {
    let mut args = env::args_os().skip(1);
    let output = match args.next().as_deref().and_then(|arg| arg.to_str()) {
        Some("--help" | "-h") => {
            println!("{USAGE}");
            return Ok(true);
        }
        Some("--check") => {
            let path = args
                .next()
                .filter(|_| args.next().is_none())
                .ok_or_else(|| io::Error::other(USAGE))?;
            return check(Path::new(&path));
        }
        Some("--output") => args
            .next()
            .map(PathBuf::from)
            .filter(|_| args.next().is_none()),
        Some(option) if option.starts_with("--output=") && args.next().is_none() => {
            Some(PathBuf::from(&option["--output=".len()..]))
        }
        _ => None,
    }
    .ok_or_else(|| io::Error::other(USAGE))?;
    let started = Instant::now();
    let root = env::var_os("RUNNER_TEMP")
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .canonicalize()?;
    let before = process::disk(&root)?;
    let tools: serde_json::Map<String, Value> = [
        "git", "python3", "python", "clang", "cmake", "ninja", "pwsh", "cl",
    ]
    .into_iter()
    .map(|tool| (tool.to_owned(), json!(lookup(tool))))
    .collect();
    let scratch = root.join("numera-engine-feasibility");
    ensure_scratch(&scratch)?;
    let after = process::disk(&root)?;
    let hosted = report::validate_runner(true).is_ok();
    let reason = if after.free < REQUIRED_FREE {
        DISK_BLOCK
    } else if hosted {
        "Cold native preflight initialized"
    } else {
        "An authorized GitHub-hosted runner is required; source checkout and compilation were not attempted."
    };
    let repo = env::var("GITHUB_REPOSITORY").unwrap_or_else(|_| "None".into());
    let run = env::var("GITHUB_RUN_ID").ok();
    let url = format!(
        "https://github.com/{repo}/actions/runs/{}",
        run.as_deref().unwrap_or("None")
    );
    let mut evidence = json!({
        "schema_revision": 1,
        "timestamp": report::now(),
        "engine": "chromium",
        "result": "BLOCKED",
        "stage": "resource-preflight",
        "runner_os": env::var("RUNNER_OS").unwrap_or_else(|_| Host::current().map_or("Unknown", Host::name).to_owned()),
        "runner_arch": env::var("RUNNER_ARCH").unwrap_or_else(|_| machine()),
        "runner_image": env::var("ImageOS").ok(),
        "execution_context": if hosted { "github-hosted" } else { "local-observation-only" },
        "runner_image_version": env::var("ImageVersion").ok(),
        "os_description": description(),
        "cpu_count": logical_cpus(),
        "commit": env::var("GITHUB_SHA").ok(),
        "run_id": run,
        "run_url": url,
        "tools": tools,
        "cache": "cold; no cache restored",
        "cleanup": "task-owned scratch only; no system/SDK cleanup",
        "disk_total_bytes": before.total,
        "disk_free_before_bytes": before.free,
        "disk_free_after_cleanup_bytes": after.free,
        "required_free_bytes": REQUIRED_FREE,
        "requirement_source": "https://chromium.googlesource.com/chromium/src/+/main/docs/windows_build_instructions.md",
        "source_acquisition_bytes": null,
        "compile_seconds": null,
        "link_seconds": null,
        "artifact_bytes": null,
        "peak_engine_rss_bytes": null,
        "peak_engine_disk_bytes": null,
        "cold_build": "NOT_RUN",
        "warm_build": "NOT_RUN",
        "reason": reason,
    });
    if hosted && after.free >= REQUIRED_FREE {
        evidence["stage"] = json!("source-acquisition");
        if let Err(error) = observe(&mut evidence, &root, &scratch, after.free) {
            evidence["reason"] = json!(format!("Cold source preflight stopped: {error}"));
        }
    }
    evidence["duration_seconds"] = json!(seconds(started.elapsed()));
    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    report::write_json(&output, &evidence)?;
    let reason = evidence["reason"].as_str().unwrap_or_default();
    // The original command emits its warning on stdout for hosted logs.
    let timestamp = format!("{:.6}", jiff::Timestamp::now()).replace('Z', "+00:00");
    println!(
        "{{\"timestamp\": \"{timestamp}\", \"severity\": \"WARNING\", \"component\": \"engine-feasibility\", \"message\": {}}}",
        serde_json::to_string(reason)?
    );
    if let Some(summary) = env::var_os("GITHUB_STEP_SUMMARY") {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(summary)?;
        let stage = evidence["stage"].as_str().unwrap_or_default();
        writeln!(
            file,
            "### Chromium source-build gate: BLOCKED\n\nStage: `{stage}`. Free: {:.2} GB; required safety budget: 100 GB.\n\n{reason} Consult JSON for attempted stages; missing measurements are not a PASS.",
            after.free as f64 / 1e9
        )?;
    }
    Ok(true)
}

fn check(path: &Path) -> io::Result<bool> {
    let data: Value = serde_json::from_slice(&fs::read(path)?)?;
    Ok(data["result"] == "PASS")
}

fn ensure_scratch(path: &Path) -> io::Result<()> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error)
            if error.kind() == io::ErrorKind::AlreadyExists
                && fs::symlink_metadata(path)?.is_dir() =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn observe(evidence: &mut Value, root: &Path, scratch: &Path, initial_free: u64) -> io::Result<()> {
    let start = Instant::now();
    let binary = process::binary_sibling("numera-build-chromium")?;
    let mut command = process::command(binary, &process::build_environment());
    command
        .arg("--source-root")
        .arg(scratch)
        .args(["--phase", "Build"]);
    let mut child = process::ChildWatch::spawn(&mut command)?;
    let mut min_free = initial_free;
    let mut reason = "Native recipe returned; compile/link/package provenance and real-engine acceptance remain unproven";
    let exit = loop {
        if let Some(exit) = child.try_wait()? {
            break exit;
        }
        let free = process::disk(root)?.free;
        min_free = min_free.min(free);
        let stop = if start.elapsed() >= COLD_BUDGET {
            Some("Five-minute cold spike budget exhausted")
        } else if free < DISK_GUARD {
            Some("Disk guard reached: preserve 20 GB free")
        } else {
            None
        };
        if let Some(stop) = stop {
            evidence["reason"] = json!(stop);
            reason = stop;
            break terminate(&mut child)?;
        }
        std::thread::sleep(Duration::from_secs(1));
    };
    let stage = fs::read_to_string(scratch.join("phase.txt"))
        .map(|stage| stage.trim_start_matches('\u{feff}').trim().to_owned())
        .unwrap_or_else(|_| "source-acquisition".into());
    evidence["stage"] = json!(if exit.success() {
        "native-acceptance-and-packaging"
    } else {
        &stage
    });
    evidence["source_and_build_attempt_seconds"] = json!(seconds(start.elapsed()));
    evidence["source_attempt_exit_code"] = json!(exit_code(exit));
    evidence["source_acquisition_bytes"] = json!(directory_bytes(scratch)?);
    evidence["peak_volume_disk_delta_bytes"] = json!(initial_free.saturating_sub(min_free));
    evidence["disk_measurement_note"] =
        json!("Sampled volume delta, not an exact process-tree working set");
    evidence["reason"] = json!(reason);
    Ok(())
}

fn seconds(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1_000.0).round() / 1_000.0
}

fn directory_bytes(root: &Path) -> io::Result<u64> {
    let mut bytes = 0_u64;
    let mut queue = vec![root.to_owned()];
    while let Some(dir) = queue.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                queue.push(entry.path());
            } else if kind.is_file() {
                bytes = bytes.saturating_add(entry.metadata()?.len());
            }
        }
    }
    Ok(bytes)
}

#[cfg(unix)]
fn terminate(child: &mut process::ChildWatch) -> io::Result<ExitStatus> {
    use rustix::process::{Pid, Signal, kill_process_group};
    let pid = i32::try_from(child.pid())
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| io::Error::other("Unable to identify owned source build"))?;
    if let Some(exit) = child.try_wait()? {
        return Ok(exit);
    }
    let _ = kill_process_group(pid, Signal::TERM);
    match child.wait(Duration::from_secs(10)) {
        Err(error) if error.kind() == io::ErrorKind::TimedOut => {
            let _ = kill_process_group(pid, Signal::KILL);
            child.wait(Duration::from_secs(10))
        }
        result => result,
    }
}

#[cfg(windows)]
fn terminate(child: &mut process::ChildWatch) -> io::Result<ExitStatus> {
    if let Some(exit) = child.try_wait()? {
        return Ok(exit);
    }
    let mut taskkill = Command::new("taskkill.exe");
    taskkill.args(["/PID", &child.pid().to_string(), "/T", "/F"]);
    let _ = process::capture(&mut taskkill, Duration::from_secs(10), 65_536);
    child.wait(Duration::from_secs(10))
}

fn exit_code(status: ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .or_else(|| status.signal().map(|signal| -signal))
    }
    #[cfg(not(unix))]
    status.code()
}

fn lookup(tool: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    #[cfg(windows)]
    let extensions = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    for root in env::split_paths(&paths) {
        #[cfg(windows)]
        for extension in extensions.split(';') {
            let candidate = root.join(format!("{tool}{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        #[cfg(unix)]
        {
            use rustix::fs::{Access, access};
            let candidate = root.join(tool);
            if candidate.is_file() && access(&candidate, Access::EXEC_OK).is_ok() {
                return Some(candidate);
            }
        }
    }
    None
}

fn machine() -> String {
    #[cfg(windows)]
    {
        env::var("PROCESSOR_ARCHITECTURE").unwrap_or_else(|_| env::consts::ARCH.to_owned())
    }
    #[cfg(not(windows))]
    env::consts::ARCH.to_owned()
}

fn logical_cpus() -> usize {
    // Host measurements include online CPUs outside this process's affinity.
    #[cfg(target_os = "linux")]
    if let Ok(contents) = fs::read_to_string("/proc/stat") {
        let online = contents
            .lines()
            .filter(|line| {
                line.strip_prefix("cpu")
                    .and_then(|rest| rest.as_bytes().first())
                    .is_some_and(u8::is_ascii_digit)
            })
            .count();
        if online > 0 {
            return online;
        }
    }
    process::cpu_count()
}

fn description() -> String {
    #[cfg(target_os = "linux")]
    {
        let os = rustix::system::uname();
        let mut description = format!(
            "{}-{}-{}",
            os.sysname().to_string_lossy(),
            os.release().to_string_lossy(),
            os.machine().to_string_lossy()
        );
        if let Ok(bytes) = process::stdout(
            Command::new("getconf").arg("GNU_LIBC_VERSION"),
            Duration::from_secs(2),
            1024,
        ) && let Ok(text) = std::str::from_utf8(&bytes)
            && let Some(version) = text.trim().strip_prefix("glibc ")
        {
            description.push_str("-with-glibc");
            description.push_str(version);
        }
        description
    }
    #[cfg(windows)]
    {
        process::stdout(
            Command::new("cmd.exe").args(["/D", "/C", "ver"]),
            Duration::from_secs(2),
            1024,
        )
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
        .unwrap_or_else(|| "Windows".into())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::{check, directory_bytes, ensure_scratch};

    #[test]
    fn measurements_exclude_symlinks_and_scratch_must_be_a_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let scratch = temporary.path().join("scratch");
        ensure_scratch(&scratch).unwrap();
        let item = scratch.join("fixture");
        std::fs::write(&item, b"program").unwrap();
        let alias = scratch.join("alias");
        std::os::unix::fs::symlink(item, alias).unwrap();
        let foreign = temporary.path().join("foreign");
        std::os::unix::fs::symlink(&scratch, &foreign).unwrap();
        assert!(ensure_scratch(&foreign).is_err());
        assert_eq!(directory_bytes(&scratch).unwrap(), 7);
    }

    #[test]
    fn only_a_parsed_explicit_pass_unlocks_the_gate() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("report.json");
        for (text, passed) in [
            (r#"{"result":"PASS"}"#, true),
            (r#"{"result":"BLOCKED"}"#, false),
            (r#"{"result":true}"#, false),
            (r#"{}"#, false),
        ] {
            std::fs::write(&path, text).unwrap();
            assert_eq!(check(&path).unwrap(), passed);
        }
        std::fs::write(&path, b"bad JSON").unwrap();
        assert!(check(&path).is_err());
    }
}
