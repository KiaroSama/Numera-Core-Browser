use super::{Result, Stage, outputs};
use core_tooling::{
    chromium_build::{
        cache,
        metadata::{Host, windows_object_probe_targets},
        process::{self, Environment},
        report::{self, CheckRun, getenv},
        tools::{self, Toolset},
    },
    chromium_license_inventory::prepare_candidate_lock,
    fixtures::read_source_export,
};
use serde_json::{Value, json};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

#[path = "stages/continuation.rs"]
mod continuation;
#[path = "stages/native.rs"]
mod native;

struct Graph {
    source: PathBuf,
    native_output: PathBuf,
    lock: PathBuf,
    context: Value,
    targets: Vec<String>,
    source_proof: Option<(PathBuf, Value)>,
}

fn graph(
    stage: &Stage,
    tools: &Toolset,
    environment: &Environment,
    config: &Path,
    evidence: &mut Value,
) -> Result<Graph> {
    let mut lock = PathBuf::from("upstream/chromium/lock.json");
    let selected = getenv("REVIEWED_UPSTREAM_EVIDENCE");
    if !selected.is_empty() {
        let current = serde_json::from_slice(&fs::read(&lock)?)?;
        let (candidate, _, record) = prepare_candidate_lock(&current, &selected, &stage.output)?;
        lock = candidate;
        evidence["candidate_source_review"] = record;
    }
    let lock_sha = cache::sha256(&lock)?;
    if let Some(base) = &stage.base
        && base.context["source_lock_sha256"].as_str() != Some(&lock_sha)
    {
        return Err("The reviewed checkpoint belongs to a different Chromium source lock".into());
    }
    let selected = getenv("VERIFIED_SOURCE_EXPORT");
    let source_proof = if selected.is_empty() {
        None
    } else {
        Some(read_source_export(&selected, stage.host.name(), &lock_sha)?)
    };
    let review_lock = (lock != Path::new("upstream/chromium/lock.json")).then_some(lock.as_path());
    tools::compile_graph(
        &stage.root,
        environment,
        &stage.output,
        tools.cache_tool,
        review_lock,
    )?;
    let source = stage.root.join("checkout/src");
    let native_output = source.join("out/Numera");
    let targets = stage.host.targets();
    let commands = stage.tools.join("ninja-commands.txt");
    let mut command = process::command(&tools.ninja, environment);
    command
        .args(["-C", "out/Numera", "-t", "commands"])
        .args(&targets)
        .current_dir(&source)
        .stdout(fs::File::create_new(&commands)?)
        .stderr(Stdio::null())
        .stdin(Stdio::null());
    if !process::ChildWatch::spawn(&mut command)?
        .wait(Duration::from_secs(120))?
        .success()
        || !(1..=1024_u64.pow(3)).contains(&commands.metadata()?.len())
    {
        return Err("Unexpected full Ninja command inventory size or command failure".into());
    }
    let clang = source
        .join("third_party/llvm-build/Release+Asserts/bin")
        .join(if stage.host == Host::Windows {
            "clang-cl.exe"
        } else {
            "clang"
        });
    let mut context = json!({"head":getenv("GITHUB_SHA"),"run_id":getenv("GITHUB_RUN_ID"),
        "run_attempt":getenv("GITHUB_RUN_ATTEMPT"),"runner_image":getenv("ImageVersion"),
        "host":stage.host.name(),"scratch":stage.root,"targets":targets,"source_lock_sha256":lock_sha,
        "gn_args_sha256":cache::sha256(&native_output.join("args.gn"))?,"commands_sha256":cache::sha256(&commands)?,
        "cache_tool":tools.cache_tool,"cache_tool_sha256":cache::sha256(&tools.cache)?,
        "cache_config_sha256":cache::sha256(config)?,"ninja_sha256":cache::sha256(&tools.ninja)?,"clang_sha256":cache::sha256(&clang)?});
    if stage.host == Host::Windows {
        let sdk = fs::read(stage.root.join("windows-sdk.json"))?;
        let sdk: Value = serde_json::from_slice(sdk.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&sdk))?;
        context["sdk"] = json!({"directory_version":sdk["directory_version"],"debugging_tools_version":sdk["debugging_tools_version"],
            "requested_release":sdk["requested_release"]});
        context["sccache_policy"] = json!({"SCCACHE_CACHE_SIZE":"4G","SCCACHE_DIRECT":"false"});
    }
    evidence["context"] = context.clone();
    fs::remove_file(commands)?;
    Ok(Graph {
        source,
        native_output,
        lock,
        context,
        targets,
        source_proof,
    })
}

fn probe_targets(graph: &Graph, tools: &Toolset, environment: &Environment) -> Result<Vec<String>> {
    let mut command = process::command(&tools.ninja, environment);
    command
        .args(["-C", "out/Numera", "-t", "compdb-targets"])
        .args(&graph.targets)
        .current_dir(&graph.source);
    let contents = process::stdout(&mut command, Duration::from_secs(120), 1024_u64.pow(3))?;
    Ok(windows_object_probe_targets(&serde_json::from_slice(
        &contents,
    )?)?)
}

pub fn execute(stage: &Stage, evidence: &mut Value, check: &mut Option<CheckRun>) -> Result<()> {
    let mut environment = report::engine_env(&stage.tools, &stage.root)?;
    let tools = tools::install_tools(&stage.tools, stage.host, &environment)?;
    let config =
        tools::configure_cache(&mut environment, &tools, &stage.tools, &stage.entries, true)?;
    let graph = graph(stage, &tools, &environment, &config, evidence)?;
    let mut entries = continuation::restore(stage, &graph.context, evidence)?;
    if tools.cache_tool == "ccache" && (stage.segment > 1 || stage.base.is_some()) {
        tools::cache_call(&tools, &environment, &["--cleanup"])?;
    }
    let targets = if stage.object_probe {
        let selected = probe_targets(&graph, &tools, &environment)?;
        evidence["object_probe"] = json!({"selected_targets":selected,"cache_retained":false});
        selected
    } else {
        graph.targets.clone()
    };
    let delta_directory = std::env::current_dir()?.join("build/chromium-object-delta");
    let expected = continuation::object_resume(
        stage,
        &graph.context,
        &delta_directory,
        &targets,
        &mut entries,
        &mut environment,
        evidence,
    )?;
    evidence["stage"] = json!(if stage.object_probe {
        "compile-objects"
    } else {
        "compile-link"
    });
    evidence["source_build"] = json!("RUNNING");
    evidence["result"] = json!("RUNNING");
    let (reason, values) = tools::cache_runtime(&tools, &environment, || {
        let reason = native::compile(
            stage,
            &graph.source,
            &tools,
            &environment,
            &targets,
            evidence,
            check,
        )?;
        Ok((reason, tools::statistics(&tools, &environment)?))
    })?;
    evidence["segment_stop"] = json!(reason);
    evidence["compiler_cache"] = values.clone();
    if (stage.segment > 1 || stage.base.is_some())
        && values["preprocessed_cache_hit"]
            .as_u64()
            .unwrap_or_default()
            < 1
    {
        return Err("Fresh runner did not reuse any transferred Chromium compile outputs".into());
    }
    if stage.object_probe {
        if reason != "built" {
            return Err("The targeted compiler probe did not finish".into());
        }
        let probe = continuation::ObjectProbe {
            directory: &delta_directory,
            targets: &targets,
            cache: &values,
            expected,
        };
        return continuation::object_finish(stage, &graph, &entries, probe, evidence);
    }
    if reason == "time-budget" {
        return continuation::checkpoint(stage, &graph.context, &entries, &values, evidence);
    }
    native::accept(stage, &graph, &mut environment, evidence)?;
    evidence["result"] = json!("PASS");
    evidence["stage"] = json!("source-and-native-fixtures-complete");
    outputs("PASS", "", "", "")
}

fn read_json_bounded(path: &Path, maximum: u64) -> Result<Value> {
    let file = fs::File::open(path)?;
    if file.metadata()?.len() > maximum {
        return Err("Native JSON exceeds its size limit".into());
    }
    Ok(serde_json::from_reader(file.take(maximum + 1))?)
}
