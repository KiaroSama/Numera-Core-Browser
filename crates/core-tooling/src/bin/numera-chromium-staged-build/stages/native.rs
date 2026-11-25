use super::{Graph, Result, Stage};
use core_tooling::{
    chromium_build::{
        cache,
        metadata::{Host, compiler_workers},
        process::{self, Environment},
        report::{self, CheckRun},
        tools::{self, Toolset},
    },
    chromium_license_inventory::linked_build_materials,
    fixtures::{
        install_linux_sandbox, retain_development_bundle, smoke_driver, smoke_headed_browser,
        smoke_portable_runtime, smoke_renderer,
    },
};
use serde_json::{Value, json};
use std::{
    io,
    path::Path,
    thread,
    time::{Duration, Instant},
};

pub fn compile(
    stage: &Stage,
    source: &Path,
    toolset: &Toolset,
    env: &Environment,
    targets: &[String],
    evidence: &mut Value,
    check: &mut Option<CheckRun>,
) -> io::Result<&'static str> {
    let workers = compiler_workers(
        process::cpu_count(),
        process::memory()?.available,
        stage.windows_delta,
    );
    evidence["parallelism"] = json!(workers);
    tools::cache_call(toolset, env, &["--zero-stats"])?;
    let mut command = process::command(&toolset.ninja, env);
    command
        .args(["-C", "out/Numera", "-j", &workers.to_string()])
        .args(targets)
        .current_dir(source);
    let log_path = stage.output.join("native-build.log");
    let started = Instant::now();
    let mut child = process::stream_log(&mut command, &log_path)?;
    let mut next_report = Duration::ZERO;
    let outcome = loop {
        if let Some(status) = child.try_wait()? {
            break if status.success() {
                Ok("built")
            } else {
                Err(io::Error::other(
                    "Native Chromium build failed; inspect native-build.log",
                ))
            };
        }
        let elapsed = started.elapsed();
        let free = process::disk(source)?.free;
        if free < 25_000_000_000 {
            break Err(io::Error::other(
                "Staged build reached its 25 GB free disk floor",
            ));
        }
        if elapsed >= Duration::from_secs(stage.minutes * 60) {
            break Ok("time-budget");
        }
        if elapsed >= next_report {
            let stats = tools::statistics(toolset, env)?;
            let (rss, _) = process::process_tree(child.pid());
            evidence["elapsed_seconds"] = json!(elapsed.as_secs_f64());
            evidence["disk_free_bytes"] = json!(free);
            evidence["native_action_progress"] = json!(report::read_progress(&log_path));
            evidence["peak_tree_rss_bytes"] =
                json!(rss.max(evidence["peak_tree_rss_bytes"].as_u64().unwrap_or_default()));
            evidence["compiler_cache"] = stats.clone();
            report::write_json(&stage.output.join("report.json"), evidence)?;
            report::publish_check(evidence, check);
            report::log(
                "INFO",
                "staged-chromium",
                &format!(
                    "Native build: {:.0}s, {} cache hits, {} writes, {:.2} GB free",
                    elapsed.as_secs_f64(),
                    stats["preprocessed_cache_hit"],
                    stats["local_storage_write"],
                    free as f64 / 1e9
                ),
            );
            next_report = elapsed + Duration::from_secs(30);
        }
        thread::sleep(Duration::from_secs(2));
    };
    drop(child);
    evidence["elapsed_seconds"] = json!(started.elapsed().as_secs_f64());
    evidence["native_action_progress"] = json!(report::read_progress(&log_path));
    outcome
}

fn sdk_clients(
    binary: &Path,
    driver: &Path,
    stage: &Stage,
    environment: &Environment,
) -> Result<Value> {
    let executable = process::binary_sibling("numera-automation-baseline")?;
    let mut command = process::command(executable, environment);
    command
        .arg("--sdk-clients")
        .args([binary, driver, &stage.root, &stage.output]);
    if let Some(adapter) = std::env::var_os("NUMERA_SDK_ADAPTER").filter(|value| !value.is_empty())
    {
        command.arg("--sdk-adapter").arg(adapter);
    }
    let captured = process::stdout(&mut command, Duration::from_secs(250), 4 * 1024 * 1024)?;
    let result: Value = serde_json::from_slice(&captured)?;
    if !matches!(result["result"].as_str(), Some("PASS" | "NOT_RUN")) {
        return Err("Native automation client results are incomplete".into());
    }
    Ok(result)
}

pub fn accept(
    stage: &Stage,
    graph: &Graph,
    environment: &mut Environment,
    evidence: &mut Value,
) -> Result<()> {
    let browser = graph
        .native_output
        .join(format!("chrome{}", std::env::consts::EXE_SUFFIX));
    let driver = graph
        .native_output
        .join(format!("chromedriver{}", std::env::consts::EXE_SUFFIX));
    evidence["stage"] = json!("renderer-fixture");
    evidence["source_build"] = json!("PASS");
    evidence["renderer_fixture"] = json!("RUNNING");
    evidence["browser_sha256"] = json!(cache::sha256(&browser)?);
    evidence["browser_bytes"] = json!(browser.metadata()?.len());
    evidence["chromedriver_sha256"] = json!(cache::sha256(&driver)?);
    evidence["chromedriver_bytes"] = json!(driver.metadata()?.len());
    if stage.host == Host::Linux {
        evidence["linux_sandbox_helper"] =
            install_linux_sandbox(&graph.native_output, &stage.root, environment)?;
    }
    smoke_renderer(&browser, &stage.root, &stage.output, environment)?;
    evidence["renderer_fixture"] = json!("PASS");
    evidence["stage"] = json!("webdriver-fixture");
    evidence["driver_fixture"] = json!("RUNNING");
    evidence["webdriver"] = smoke_driver(
        &browser,
        &driver,
        &stage.root,
        &stage.output,
        environment,
        false,
    )?;
    evidence["driver_fixture"] = json!("PASS");
    evidence["stage"] = json!("automation-client-fixtures");
    evidence["automation_client_smokes"] = json!("RUNNING");
    evidence["automation_clients"] = sdk_clients(&browser, &driver, stage, environment)?;
    evidence["automation_client_smokes"] = evidence["automation_clients"]["result"].clone();
    evidence["stage"] = json!("portable-runtime-fixture");
    evidence["portable_runtime_fixture"] = json!("RUNNING");
    evidence["portable_runtime"] = smoke_portable_runtime(
        &graph.source,
        &stage.root,
        &stage.output,
        stage.host.name(),
        environment,
    )?;
    evidence["portable_runtime_fixture"] = json!("PASS");
    evidence["stage"] = json!("headed-browser-fixture");
    evidence["headed_browser_fixture"] = json!("RUNNING");
    evidence["headed_browser"] =
        smoke_headed_browser(&browser, &driver, &stage.root, &stage.output, environment)?;
    evidence["headed_browser_fixture"] = json!("PASS");
    evidence["stage"] = json!("linked-build-materials");
    evidence["linked_build_materials"] = json!("RUNNING");
    evidence["build_materials"] = linked_build_materials(
        &stage.root,
        &graph.source,
        &stage.output.join("linked-build-materials"),
        stage.host.name(),
        environment,
        &graph.lock,
    )?;
    let digests = &evidence["build_materials"]["inputs_sha256"];
    if digests["chromium-source-lock.json"] != graph.context["source_lock_sha256"]
        || digests["chromium-args.gn"] != graph.context["gn_args_sha256"]
    {
        return Err("Retained source material does not match the compiled build context".into());
    }
    evidence["linked_build_materials"] = json!("PASS");
    if let Some(proof) = &graph.source_proof {
        evidence["stage"] = json!("development-runtime-bundle");
        if evidence["automation_client_smokes"] != "PASS" {
            return Err("A verified external SDK adapter is required to retain a source-built development runtime".into());
        }
        evidence["development_runtime"] =
            retain_development_bundle(&stage.root, &stage.output, evidence, proof, environment)?;
    }
    Ok(())
}
