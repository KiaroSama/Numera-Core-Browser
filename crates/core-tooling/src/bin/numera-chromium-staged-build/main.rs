mod stages;

use core_tooling::chromium_build::{
    metadata::{Host, ReviewedBase, ReviewedDelta, SEGMENTS, reviewed_checkpoint, reviewed_delta},
    process,
    report::{self, CheckRun, getenv},
};
use serde_json::{Value, json};
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const DEFAULT_MINUTES: u64 = 275;
const MAX_MINUTES: u64 = 305;

struct Arguments {
    segment: u32,
    minutes: u64,
    checkpoint_key: bool,
    delta_id: bool,
}

impl Arguments {
    fn parse() -> Result<Option<Self>> {
        let mut options = Self {
            segment: 0,
            minutes: DEFAULT_MINUTES,
            checkpoint_key: false,
            delta_id: false,
        };
        let mut args = std::env::args().skip(1);
        while let Some(flag) = args.next() {
            let (flag, inline) = match flag.split_once('=') {
                Some((name @ ("--segment" | "--minutes"), value)) => (name, Some(value)),
                _ => (flag.as_str(), None),
            };
            let mut value = || inline.map(str::to_owned).or_else(|| args.next());
            match flag {
                "--segment" => options.segment = value().ok_or("Missing segment")?.parse()?,
                "--minutes" => {
                    options.minutes = value().ok_or("Missing compile minutes")?.parse()?
                }
                "--checkpoint-key" => options.checkpoint_key = true,
                "--delta-artifact-id" => options.delta_id = true,
                "-h" | "--help" => {
                    println!(
                        "Usage: numera-chromium-staged-build --segment 1..4 [--minutes 10..305] [--checkpoint-key | --delta-artifact-id]"
                    );
                    return Ok(None);
                }
                _ => return Err("Unknown staged build argument".into()),
            }
        }
        if !(1..=SEGMENTS).contains(&options.segment)
            || !(10..=MAX_MINUTES).contains(&options.minutes)
        {
            return Err("Staged build requires a valid segment and time budget".into());
        }
        Ok(Some(options))
    }
}

struct Stage {
    host: Host,
    segment: u32,
    minutes: u64,
    windows_delta: bool,
    read_only: bool,
    object_probe: bool,
    probe_delta: String,
    base: Option<ReviewedBase>,
    inherited_delta: Option<ReviewedDelta>,
    output: PathBuf,
    transfer: PathBuf,
    root: PathBuf,
    tools: PathBuf,
    entries: PathBuf,
}

impl Stage {
    fn new(options: &Arguments, host: Host) -> Result<Self> {
        let run = getenv("REVIEWED_CHECKPOINT_RUN");
        let windows_delta = getenv("WINDOWS_STAGED_DELTA") == "true";
        let read_only = getenv("READ_ONLY_REPLAY") == "true";
        let object_probe = getenv("WINDOWS_OBJECT_PROBE") == "true";
        let probe_delta = getenv("WINDOWS_OBJECT_DELTA");
        let evidence = Path::new("docs/evidence");
        let base = if run.is_empty() {
            None
        } else {
            if options.segment != 1 && !windows_delta {
                return Err("Only the first segment can start from a reviewed earlier run".into());
            }
            Some(reviewed_checkpoint(
                evidence,
                &run,
                getenv("REVIEWED_CHECKPOINT_SEGMENT").parse()?,
                host,
            )?)
        };
        if read_only && base.is_none() {
            return Err("A read-only replay requires a reviewed earlier checkpoint".into());
        }
        if object_probe
            && (!read_only || host != Host::Windows || !getenv("VERIFIED_SOURCE_EXPORT").is_empty())
        {
            return Err(
                "The object probe requires a Windows read-only replay without runtime retention"
                    .into(),
            );
        }
        if !probe_delta.is_empty()
            && (!object_probe || !matches!(probe_delta.as_str(), "seed" | "resume"))
        {
            return Err("The compiler cache transfer requires a Windows object probe".into());
        }
        if windows_delta && (host != Host::Windows || base.is_none() || read_only) {
            return Err(
                "A staged compiler cache delta requires a reviewed Windows base checkpoint".into(),
            );
        }
        if options.minutes > DEFAULT_MINUTES && !windows_delta {
            return Err(
                "The extended compile budget requires a reviewed Windows checkpoint".into(),
            );
        }
        let inherited_run = getenv("REVIEWED_DELTA_RUN");
        let inherited_delta = if inherited_run.is_empty() {
            None
        } else {
            if !windows_delta || options.segment != 1 {
                return Err(
                    "Only a first Windows segment can resume a reviewed delta artifact".into(),
                );
            }
            Some(reviewed_delta(
                evidence,
                &inherited_run,
                getenv("REVIEWED_DELTA_SEGMENT").parse()?,
                base.as_ref().ok_or("A reviewed base is required")?,
            )?)
        };
        let (output, transfer, root, tools, entries) = if options.checkpoint_key || options.delta_id
        {
            (
                PathBuf::new(),
                PathBuf::new(),
                PathBuf::new(),
                PathBuf::new(),
                PathBuf::new(),
            )
        } else {
            let project = std::env::current_dir()?;
            fs::create_dir_all(project.join("build"))?;
            let output = project.join("build/chromium-staged-evidence");
            let transfer = project.join("build/chromium-staged-cache");
            fs::create_dir(&output)?;
            fs::create_dir_all(&transfer)?;
            let root = report::required_path("RUNNER_TEMP")?.join("numera-chromium-staged");
            let tools = root.with_file_name("numera-chromium-staged-tools");
            let entries = tools.join("entries");
            fs::create_dir(&tools)?;
            fs::create_dir(&entries)?;
            (output, transfer, root, tools, entries)
        };
        Ok(Self {
            host,
            segment: options.segment,
            minutes: options.minutes,
            windows_delta,
            read_only,
            object_probe,
            probe_delta,
            base,
            inherited_delta,
            output,
            transfer,
            root,
            tools,
            entries,
        })
    }

    fn report(&self) -> Result<Value> {
        let free = process::disk(self.root.parent().ok_or("Missing runner scratch parent")?)?.free;
        let mut report = json!({"schema_revision":1,"result":"BLOCKED","stage":"prepare","segment":self.segment,
            "commit":getenv("GITHUB_SHA"),"host":self.host.name(),"release_ready":false,"source_build":"NOT_RUN",
            "read_only_replay":self.read_only,"renderer_fixture":"NOT_RUN","driver_fixture":"NOT_RUN",
            "automation_client_smokes":"NOT_RUN","automation_suites":"NOT_RUN","portable_runtime_fixture":"NOT_RUN",
            "headed_browser_fixture":"NOT_RUN","linked_build_materials":"NOT_RUN","compile_budget_minutes":self.minutes,
            "debug_build":true,"elapsed_seconds":0,"disk_free_bytes":free,"peak_tree_rss_bytes":0});
        if self.windows_delta {
            report["checkpoint_transport"] = json!("artifact-delta");
        }
        Ok(report)
    }

    fn telemetry(&self) -> Option<CheckRun> {
        let job = if self.probe_delta.is_empty() {
            ["first", "second", "third", "fourth"][self.segment as usize - 1]
        } else {
            &self.probe_delta
        };
        CheckRun::new(format!("{job} / native"))
    }
}

fn outputs(result: &str, checkpoint: &str, delta: &str, objects: &str) -> Result<()> {
    report::github_output(
        &[
            ("result", result),
            ("checkpoint_sha", checkpoint),
            ("delta_sha", delta),
            ("objects_sha", objects),
        ],
        true,
    )?;
    Ok(())
}

fn run(arguments: &Arguments) -> Result<()> {
    let stage = Stage::new(arguments, report::validate_runner(true)?)?;
    if arguments.delta_id {
        println!(
            "{}",
            stage
                .inherited_delta
                .ok_or("A reviewed delta is required to resolve an artifact ID")?
                .artifact_id
        );
        return Ok(());
    }
    if arguments.checkpoint_key {
        println!(
            "{}",
            stage
                .base
                .ok_or("A reviewed run is required to resolve a cache key")?
                .key
        );
        return Ok(());
    }
    let mut evidence = stage.report()?;
    let mut check = stage.telemetry();
    let outcome = stages::execute(&stage, &mut evidence, &mut check);
    if let Err(error) = &outcome {
        evidence["result"] = json!("BLOCKED");
        evidence["error_type"] = json!("NativeCommandError");
        evidence["diagnostic"] = json!(error.to_string());
        for gate in [
            "source_build",
            "renderer_fixture",
            "driver_fixture",
            "automation_client_smokes",
            "portable_runtime_fixture",
            "headed_browser_fixture",
            "linked_build_materials",
        ] {
            if evidence[gate] == "RUNNING" {
                evidence[gate] = json!("BLOCKED");
            }
        }
    }
    evidence["timestamp"] = json!(report::now());
    report::publish_check(&mut evidence, &mut check);
    report::write_json(&stage.output.join("report.json"), &evidence)?;
    outcome
}

fn main() -> ExitCode {
    match Arguments::parse() {
        Ok(None) => ExitCode::SUCCESS,
        Ok(Some(arguments)) => match run(&arguments) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                report::log("ERROR", "staged-chromium", &error.to_string());
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}
