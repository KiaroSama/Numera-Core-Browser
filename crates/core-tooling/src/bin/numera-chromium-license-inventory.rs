use core_tooling::{
    chromium_build::{report, tools},
    chromium_license_inventory::{
        chromium_notices, git_source_inventory, prepare_candidate_lock, reviewed_stable_lock,
    },
    fixtures::describe_portable_runtime,
    sha256_file,
};
use serde_json::{Value, json};
use std::{env, error::Error, fs, path::PathBuf, process::ExitCode};

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let reviewed = match (args.next(), args.next(), args.next()) {
        (None, None, None) => None,
        (Some(flag), Some(path), None) if flag == "--reviewed-candidate" => Some(
            path.into_string()
                .map_err(|_| "Review filename must be UTF-8")?,
        ),
        (Some(flag), Some(path), None) if flag == "--review-lock" => {
            let lock = serde_json::from_slice(&fs::read("upstream/chromium/lock.json")?)?;
            let evidence = serde_json::from_slice(&fs::read(path)?)?;
            println!("{}", reviewed_stable_lock(&lock, &evidence)?);
            return Ok(());
        }
        _ => {
            return Err(
                "Usage: numera-chromium-license-inventory [--reviewed-candidate FILE]".into(),
            );
        }
    };
    let runner = report::validate_runner(true)?;
    let host = runner.name();
    let root = report::required_path("RUNNER_TEMP")?.join("numera-chromium-licenses");
    let tools_path = root.with_file_name("numera-chromium-licenses-tools");
    let output = env::current_dir()?.join("build/chromium-license-evidence");
    fs::create_dir_all(output.parent().ok_or("Missing evidence parent")?)?;
    fs::create_dir(&output)?;
    fs::create_dir(&tools_path)?;
    let environment = report::engine_env(&tools_path, &root)?;
    let mut lock_path = PathBuf::from("upstream/chromium/lock.json");
    let mut lock: Value = serde_json::from_slice(&fs::read(&lock_path)?)?;
    let review_record = if let Some(file) = reviewed {
        let (path, reviewed, record) = prepare_candidate_lock(&lock, &file, &output)?;
        lock_path = path;
        lock = reviewed;
        Some(record)
    } else {
        None
    };
    let pin = &lock["platform_pins"][host.to_ascii_lowercase()];
    let revision = pin["revision"]
        .as_str()
        .ok_or("Missing reviewed source revision")?;
    let mut evidence = json!({
        "schema_revision": 1, "source_commit": env::var("GITHUB_SHA")?, "host": host,
        "runner_image": env::var("ImageVersion")?, "chromium_revision": revision,
        "chromium_version": pin["version"], "source_lock_sha256": sha256_file(&lock_path)?,
        "targets": {}, "engine_compilation": "NOT_RUN", "redistribution_approval": false, "result": "BLOCKED",
    });
    if let Some(record) = review_record.as_ref() {
        evidence["candidate_source_review"] = record.clone();
    }
    let result: Result<(), Box<dyn Error>> = (|| {
        let toolset = tools::install_tools(&tools_path, runner, &environment)?;
        tools::compile_graph(
            &root,
            &environment,
            &output,
            toolset.cache_tool,
            review_record.as_ref().map(|_| lock_path.as_path()),
        )?;
        let source = root.join("checkout/src");
        evidence["gn_args_sha256"] = json!(sha256_file(&source.join("out/Numera/args.gn"))?);
        evidence["license_tool_sha256"] =
            json!(sha256_file(&source.join("tools/licenses/licenses.py"))?);
        let layout = describe_portable_runtime(&source, host, &environment)?;
        let layout_file = output.join("runtime-layout.json");
        report::write_json(&layout_file, &layout)?;
        let dependencies = layout["dependencies"]
            .as_array()
            .ok_or("Missing runtime dependencies")?;
        evidence["runtime_layout"] = json!({
            "sha256": sha256_file(&layout_file)?, "paths": dependencies.len(), "file_contents": "NOT_RUN",
        });
        evidence["source_inventory"] =
            git_source_inventory(&root, &source, &output, pin, &environment)?;
        evidence["targets"] = chromium_notices(&source, &output, host, revision, &environment)?;
        evidence["result"] = json!("PASS");
        Ok(())
    })();
    evidence["timestamp"] = json!(report::now());
    report::write_json(&output.join("report.json"), &evidence)?;
    result
}

fn main() -> ExitCode {
    if let Err(error) = run() {
        report::log("ERROR", "chromium-license-inventory", &error.to_string());
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
