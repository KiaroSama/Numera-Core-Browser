//! Native build evidence, bounded Ninja log parsing, and optional CI telemetry.
use super::{
    metadata::{Host, REPOSITORY},
    process::{self, Environment},
};
use serde_json::{Value, json};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

pub fn now() -> String {
    jiff::Timestamp::now().to_string()
}

pub fn log(severity: &str, component: &str, message: &str) {
    let event = json!({"timestamp": now(), "severity": severity, "component": component, "message": message});
    let _ = writeln!(io::stderr().lock(), "{event}");
}

pub fn write_json(path: &Path, value: &Value) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), value)?;
    temporary.as_file_mut().write_all(b"\n")?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

pub fn getenv(name: &str) -> String {
    env::var(name).unwrap_or_default()
}

pub fn required_path(name: &str) -> io::Result<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            io::Error::other(format!(
                "Required hosted environment variable {name} is missing"
            ))
        })
}

pub fn validate_runner(check_repository_visibility: bool) -> io::Result<Host> {
    let error = || {
        io::Error::other(
            "Native Chromium builds require the authorized public Windows or Linux x86-64 hosted job",
        )
    };
    if getenv("GITHUB_ACTIONS") != "true"
        || getenv("RUNNER_ENVIRONMENT") != "github-hosted"
        || getenv("GITHUB_REPOSITORY") != REPOSITORY
        || getenv("RUNNER_ARCH") != "X64"
    {
        return Err(error());
    }
    if check_repository_visibility {
        let data = fs::read(required_path("GITHUB_EVENT_PATH")?)?;
        let event: Value = serde_json::from_slice(&data)?;
        if event["repository"]["private"] != false {
            return Err(error());
        }
    }
    Host::current()
}

pub fn github_output(fields: &[(&str, &str)], required: bool) -> io::Result<()> {
    let output = match env::var_os("GITHUB_OUTPUT") {
        Some(path) => PathBuf::from(path),
        None if required => {
            return Err(io::Error::other(
                "GITHUB_OUTPUT is missing from the hosted job",
            ));
        }
        None => return Ok(()),
    };
    let mut file = OpenOptions::new().create(true).append(true).open(output)?;
    for (key, value) in fields {
        writeln!(file, "{key}={value}")?;
    }
    Ok(())
}

fn counter(value: &[u8]) -> Option<u64> {
    if !(1..=9).contains(&value.len()) || !value.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(value).ok()?.parse().ok()
}

fn progress_in_line(line: &[u8]) -> Option<(u64, u64)> {
    let marker = line.iter().position(|byte| !matches!(byte, b' ' | b'\t'))?;
    let rest = line.get(marker..)?.strip_prefix(b"[")?;
    let bracket = rest.iter().position(|byte| *byte == b']')?;
    if !matches!(rest.get(bracket + 1), Some(b' ' | b'\t')) {
        return None;
    }
    let divider = rest[..bracket].iter().position(|byte| *byte == b'/')?;
    let completed = counter(&rest[..divider])?;
    let total = counter(&rest[divider + 1..bracket])?;
    (total > 0 && completed <= total).then_some((completed, total))
}

pub fn read_progress(path: &Path) -> Option<Value> {
    const LIMIT: u64 = 65_536;
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(LIMIT)))
        .ok()?;
    let mut tail = Vec::new();
    file.take(LIMIT).read_to_end(&mut tail).ok()?;
    tail.rsplit(|byte| matches!(byte, b'\r' | b'\n'))
        .find_map(progress_in_line)
        .map(|(completed, total)| json!({"completed": completed, "total": total}))
}

pub fn record_stage(report: &mut Value, stage: &str) {
    report["stage"] = json!(stage);
    if matches!(stage, "compile-link" | "compile-complete") && report["source_build"] == "NOT_RUN" {
        report["source_build"] = json!("RUNNING");
    }
    if stage == "controlled-renderer-fixture" && report["renderer_fixture"] == "NOT_RUN" {
        report["renderer_fixture"] = json!("RUNNING");
    }
    if report["result"] == "BLOCKED" {
        for gate in ["source_build", "renderer_fixture"] {
            if report[gate] == "RUNNING" {
                report[gate] = json!("BLOCKED");
            }
        }
    }
}

pub struct CheckRun {
    name: String,
    id: Option<u64>,
}

impl CheckRun {
    pub fn new(name: impl Into<String>) -> Option<Self> {
        env::var_os("GH_TOKEN").filter(|token| !token.is_empty())?;
        Some(Self {
            name: name.into(),
            id: None,
        })
    }

    fn gh(&self) -> Command {
        let mut command = Command::new("gh");
        command.args([
            "api",
            "--hostname",
            "github.com",
            "--header",
            "X-GitHub-Api-Version: 2026-03-10",
        ]);
        command
    }

    fn id(&mut self) -> io::Result<u64> {
        if let Some(id) = self.id {
            return Ok(id);
        }
        let run = getenv("GITHUB_RUN_ID");
        if run.is_empty() || !run.bytes().all(|value| value.is_ascii_digit()) {
            return Err(io::Error::other("Hosted telemetry run identity is absent"));
        }
        let endpoint = format!("repos/{REPOSITORY}/actions/runs/{run}/jobs?per_page=100");
        let contents = process::stdout(
            self.gh().arg(endpoint),
            Duration::from_secs(20),
            4 * 1024 * 1024,
        )?;
        let jobs: Value = serde_json::from_slice(&contents)?;
        let mut own_jobs = jobs["jobs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|job| job["name"] == self.name && job["status"] == "in_progress");
        let own = own_jobs
            .next()
            .filter(|_| own_jobs.next().is_none())
            .ok_or_else(|| io::Error::other("Source job telemetry target is ambiguous"))?;
        let origin = format!("https://api.github.com/repos/{REPOSITORY}/check-runs/");
        let id = own["check_run_url"]
            .as_str()
            .and_then(|url| url.strip_prefix(&origin))
            .and_then(|id| id.parse::<u64>().ok())
            .filter(|id| *id > 0)
            .ok_or_else(|| io::Error::other("Source job telemetry identity is invalid"))?;
        self.id = Some(id);
        Ok(id)
    }

    fn update(&mut self, report: &Value) -> io::Result<()> {
        let id = self.id()?;
        let mut payload = tempfile::NamedTempFile::new()?;
        let value = json!({"output":{"title":report["stage"],"summary":summary(report)}});
        serde_json::to_writer(&mut payload, &value)?;
        payload.flush()?;
        let endpoint = format!("repos/{REPOSITORY}/check-runs/{id}");
        process::stdout(
            self.gh()
                .args(["--method", "PATCH", &endpoint, "--input"])
                .arg(payload.path()),
            Duration::from_secs(20),
            1024 * 1024,
        )?;
        Ok(())
    }
}

fn summary(report: &Value) -> String {
    use std::fmt::Write;
    let mut body = format!(
        "Stage: `{}`\n\nElapsed: {:.0} seconds. Free disk: {:.2} GB. Peak sampled process-tree RSS: {:.2} GB.\n\nResult: **{}**. Release ready: **false**.\n\nSource compilation and a controlled renderer fixture are separate from full browser/automation/release acceptance.",
        report["stage"].as_str().unwrap_or_default(),
        report["elapsed_seconds"].as_f64().unwrap_or_default(),
        report["disk_free_bytes"].as_f64().unwrap_or_default() / 1e9,
        report["peak_tree_rss_bytes"].as_f64().unwrap_or_default() / 1e9,
        report["result"].as_str().unwrap_or_default(),
    );
    if let Some(workers) = report["parallelism"].as_u64() {
        let debug = if report["debug_build"] == true {
            "True"
        } else {
            "False"
        };
        let _ = write!(
            &mut body,
            "\n\nLocal compiler ceiling: {workers}. Debug build: {debug}."
        );
    }
    let progress = &report["native_action_progress"];
    if let (Some(done), Some(total)) = (progress["completed"].as_u64(), progress["total"].as_u64())
    {
        let _ = write!(
            &mut body,
            "\n\nObserved native action counter: {done}/{total}. Actions include generation, compilation and linking; this is not browser acceptance or a time estimate."
        );
    }
    if let Some(cache) = report["compiler_cache"]
        .as_object()
        .filter(|cache| !cache.is_empty())
    {
        let _ = write!(
            &mut body,
            "\n\nSegment {}: {} preprocessed cache hits, {} local cache writes, {} KiB in cache.",
            report["segment"],
            cache["preprocessed_cache_hit"],
            cache["local_storage_write"],
            cache["cache_size_kibibyte"]
        );
    }
    body
}

pub fn publish_check(report: &mut Value, check: &mut Option<CheckRun>) {
    if let Some(check) = check
        && check.update(report).is_err()
    {
        report["telemetry_publish_error_type"] = json!("NativeCommandError");
        log(
            "WARNING",
            "engine-build",
            "Live check output unavailable; local evidence continues",
        );
    }
}

pub fn engine_env(tools: &Path, root: &Path) -> io::Result<Environment> {
    let mut environment = process::build_environment();
    let depot = root.join("depot_tools");
    process::prepend_path(&mut environment, &[tools, &depot])?;
    for (key, value) in [
        ("DEPOT_TOOLS_UPDATE", "0"),
        ("DEPOT_TOOLS_WIN_TOOLCHAIN", "0"),
        ("NINJA_STATUS", "[%f/%t] "),
    ] {
        process::set_environment(&mut environment, key, value);
    }
    Ok(environment)
}
