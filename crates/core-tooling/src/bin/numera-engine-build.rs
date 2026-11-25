use core_tooling::{
    chromium_build::{cache::sha256, process, report},
    fixtures::{install_linux_sandbox, smoke_renderer},
};
use serde_json::{Value, json};
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
    thread,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct Options {
    output: PathBuf,
    minutes: u64,
    parallelism: usize,
    prerequisites: bool,
    component: bool,
    debug: bool,
}

fn options() -> Result<Option<Options>> {
    let mut options = Options {
        output: PathBuf::new(),
        minutes: 330,
        parallelism: 4,
        prerequisites: false,
        component: false,
        debug: false,
    };
    let mut args = std::env::args_os().skip(1);
    while let Some(flag) = args.next() {
        let (name, inline) = match flag.to_str().and_then(|text| text.split_once('=')) {
            Some((name @ ("--output" | "--budget-minutes" | "--parallelism"), value)) => {
                (Some(name), Some(value))
            }
            _ => (flag.to_str(), None),
        };
        let mut value = || {
            inline
                .map(std::ffi::OsString::from)
                .or_else(|| args.next())
                .ok_or("Missing cold-build argument value")
        };
        match name {
            Some("--help" | "-h") => {
                println!(
                    "Usage: numera-engine-build --output DIRECTORY [--budget-minutes MINUTES] [--parallelism CORES] [--component-build] [--debug-build] [--prerequisites-only]"
                );
                return Ok(None);
            }
            Some("--output") => options.output = PathBuf::from(value()?),
            Some("--budget-minutes") => {
                options.minutes = value()?.to_str().ok_or("Invalid budget")?.parse()?
            }
            Some("--parallelism") => {
                options.parallelism = value()?.to_str().ok_or("Invalid parallelism")?.parse()?
            }
            Some("--component-build") => options.component = true,
            Some("--debug-build") => options.debug = true,
            Some("--prerequisites-only") => options.prerequisites = true,
            _ => return Err("Unknown cold source-build argument".into()),
        }
    }
    if options.output.as_os_str().is_empty()
        || !(10..=330).contains(&options.minutes)
        || !(1..=4).contains(&options.parallelism)
    {
        return Err(
            "An output directory, 10 to 330 budget minutes, and 1 to 4 workers are required".into(),
        );
    }
    Ok(Some(options))
}

fn phase(path: &Path) -> Option<String> {
    fs::read_to_string(path.join("phase.txt"))
        .ok()
        .map(|text| text.trim_start_matches('\u{feff}').trim().to_owned())
}

fn run(options: &Options) -> Result<()> {
    let host = report::validate_runner(false)?;
    fs::create_dir_all(&options.output)?;
    let output = options.output.canonicalize()?;
    let run_id = report::getenv("GITHUB_RUN_ID");
    if run_id.is_empty() || !run_id.bytes().all(|value| value.is_ascii_digit()) {
        return Err("Hosted build run ID is invalid".into());
    }
    let scratch = report::required_path("RUNNER_TEMP")?.join(format!("numera-cold-{run_id}"));
    fs::create_dir(&scratch)?;
    let started = Instant::now();
    let memory = process::memory()?;
    let cpus = process::cpu_count();
    let workers = options
        .parallelism
        .min(cpus)
        .min((memory.available / (3 * 1024_u64.pow(3))) as usize)
        .max(1);
    let initial_disk = process::disk(&scratch)?;
    let lock: Value = serde_json::from_slice(&fs::read("upstream/chromium/lock.json")?)?;
    let mut evidence = json!({
        "schema_revision": 1, "timestamp": report::now(), "commit": report::getenv("GITHUB_SHA"),
        "run_id": run_id, "run_url": format!("https://github.com/{}/actions/runs/{run_id}", report::getenv("GITHUB_REPOSITORY")),
        "runner_os":host.name(), "runner_arch":"X64", "runner_image_version":report::getenv("ImageVersion"),
        "cpu_count":cpus, "physical_memory_bytes":memory.total, "telemetry_backend":"native Rust",
        "budget_minutes":options.minutes, "component_build": options.component, "debug_build": options.debug,
        "native_action_progress":null, "requested_parallelism": options.parallelism, "parallelism":workers,
        "memory_available_initial_bytes": memory.available, "prerequisites_only":options.prerequisites,
        "parallelism_policy":"At most available CPUs, requested ceiling and 3 GiB of initially available memory per local compiler; this is a conservative recipe, not a universal engine resource minimum",
        "native_source_targets": if options.prerequisites { Vec::new() } else { host.targets() },
        "disk_total_bytes":initial_disk.total,"disk_free_initial_bytes":initial_disk.free,"disk_free_bytes":initial_disk.free,
        "minimum_free_disk_bytes":initial_disk.free, "elapsed_seconds":0, "peak_tree_rss_bytes":0,"peak_tree_process_count":0,
        "stage":"resource-preflight","result":"RUNNING","release_ready":false,"cold_cache":true,
        "precompiled_engine_objects_reused":false,"source_build":"NOT_RUN","renderer_fixture":"NOT_RUN",
        "automation_suites":"NOT_RUN","source_lock":lock,
    });
    let native_log = output.join("native-build.log");
    let mut environment = process::build_environment();
    let mut check = report::CheckRun::new(report::getenv("GITHUB_JOB"));
    let mut min_free = initial_disk.free;
    let outcome = (|| -> Result<()> {
        if initial_disk.free < 100_000_000_000 {
            return Err("Conservative 100 GB source-build disk preflight failed".into());
        }
        let recipe = process::binary_sibling("numera-build-chromium")?;
        let mut command = process::command(recipe, &environment);
        command.arg("--source-root").arg(&scratch).args([
            "--phase",
            if options.prerequisites {
                "Prerequisites"
            } else {
                "Build"
            },
            "--parallelism",
            &workers.to_string(),
            "--build-driver",
        ]);
        if options.component {
            command.arg("--component-build");
        }
        if options.debug {
            command.arg("--debug-build");
        }
        let mut child = process::stream_log(&mut command, &native_log)?;
        let mut next = Duration::ZERO;
        loop {
            if let Some(status) = child.try_wait()? {
                evidence["native_exit_code"] = json!(status.code());
                if !status.success() {
                    return Err("Pinned native source-build recipe failed; inspect retained native-build.log".into());
                }
                break;
            }
            let elapsed = started.elapsed();
            let free = process::disk(&scratch)?.free;
            min_free = min_free.min(free);
            evidence["disk_free_bytes"] = json!(free);
            evidence["minimum_free_disk_bytes"] = json!(min_free);
            evidence["elapsed_seconds"] = json!(elapsed.as_secs_f64());
            if let Some(current) = phase(&scratch) {
                report::record_stage(&mut evidence, &current);
            }
            evidence["native_action_progress"] = json!(report::read_progress(&native_log));
            let (rss, processes) = process::process_tree(child.pid());
            evidence["peak_tree_rss_bytes"] =
                json!(rss.max(evidence["peak_tree_rss_bytes"].as_u64().unwrap_or_default()));
            evidence["peak_tree_process_count"] = json!(
                processes.max(
                    evidence["peak_tree_process_count"]
                        .as_u64()
                        .unwrap_or_default() as usize
                )
            );
            if elapsed >= Duration::from_secs(options.minutes * 60) {
                return Err("Bounded cold source-build time budget exhausted".into());
            }
            if free < 20_000_000_000 {
                return Err("Free disk guard reached; preserve 20 GB".into());
            }
            if elapsed >= next {
                evidence["timestamp"] = json!(report::now());
                report::write_json(&output.join("report.json"), &evidence)?;
                report::publish_check(&mut evidence, &mut check);
                let stage = evidence["stage"].as_str().unwrap_or_default();
                report::log(
                    "INFO",
                    "engine-build",
                    &format!(
                        "{stage}: {:.0}s, {:.2} GB free, compiler ceiling {workers}",
                        elapsed.as_secs_f64(),
                        free as f64 / 1e9
                    ),
                );
                next = elapsed + Duration::from_secs(30);
            }
            thread::sleep(Duration::from_secs(1));
        }
        if options.prerequisites {
            evidence["result"] = json!("PREREQUISITES_PASS");
            evidence["stage"] = json!("prerequisites-pass-source-build-not-run");
            return Ok(());
        }
        evidence["source_build"] = json!("PASS");
        report::record_stage(&mut evidence, "controlled-renderer-fixture");
        let native = scratch.join("checkout/src/out/Numera");
        let binary = native.join(format!("chrome{}", std::env::consts::EXE_SUFFIX));
        evidence["binary_sha256"] = json!(sha256(&binary)?);
        evidence["binary_bytes"] = json!(binary.metadata()?.len());
        if host.name() == "Linux" {
            evidence["linux_sandbox_helper"] =
                install_linux_sandbox(&native, &scratch, &mut environment)?;
        }
        smoke_renderer(&binary, &scratch, &output, &environment)?;
        evidence["renderer_fixture"] = json!("PASS");
        evidence["result"] = json!("PASS");
        evidence["stage"] = json!("source-and-renderer-pass-release-gates-open");
        Ok(())
    })();
    if let Err(error) = &outcome {
        evidence["result"] = json!("BLOCKED");
        evidence["error_type"] = json!("NativeCommandError");
        evidence["reason"] = json!(error.to_string());
        if evidence["source_build"] != "PASS"
            && let Some(current) = phase(&scratch)
        {
            evidence["stage"] = json!(current);
        }
    }
    let stage = evidence["stage"].as_str().unwrap_or_default().to_owned();
    report::record_stage(&mut evidence, &stage);
    evidence["timestamp"] = json!(report::now());
    evidence["elapsed_seconds"] = json!(started.elapsed().as_secs_f64());
    evidence["native_action_progress"] = json!(report::read_progress(&native_log));
    evidence["sampled_volume_delta_bytes"] = json!(initial_disk.free.saturating_sub(min_free));
    evidence["measurement_note"] = json!(
        "RSS sums sampled process-tree resident memory; shared pages may be double-counted. Volume delta is not exact engine disk allocation."
    );
    for name in [
        "phase.txt",
        "phases.ndjson",
        "gn-args.txt",
        "windows-sdk.json",
        "windows-sdk.log",
    ] {
        let path = scratch.join(name);
        if path.is_file() {
            fs::copy(path, output.join(name))?;
        }
    }
    report::publish_check(&mut evidence, &mut check);
    report::write_json(&output.join("report.json"), &evidence)?;
    outcome
}

fn main() -> ExitCode {
    match options() {
        Ok(None) => ExitCode::SUCCESS,
        Ok(Some(options)) => match run(&options) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                report::log("ERROR", "engine-build", &error.to_string());
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}
