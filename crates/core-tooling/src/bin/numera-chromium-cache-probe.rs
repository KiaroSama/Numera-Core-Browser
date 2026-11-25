use core_tooling::{
    chromium_build::{
        cache::{self, Inventory, PROBE_LIMITS, Restore},
        metadata::Host,
        process::{self, Environment},
        report::{self, getenv},
        tools::{self, Toolset},
    },
    sha256_hex,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

struct Probe {
    tools: Toolset,
    environment: Environment,
    source: PathBuf,
    directory: PathBuf,
    entries: PathBuf,
    output: PathBuf,
    target: &'static str,
}

fn ccache_results(path: &Path, target: &str) -> io::Result<Vec<String>> {
    let mut callers = BTreeSet::new();
    let mut results: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut log = BufReader::new(fs::File::open(path)?);
    let mut bytes = Vec::new();
    while log.read_until(b'\n', &mut bytes)? != 0 {
        let line = String::from_utf8_lossy(&bytes);
        if let Some((stamp, part)) = line.trim_end().split_once("] ")
            && stamp.starts_with('[')
            && let Some(pid) = stamp.split_whitespace().next_back()
            && pid.bytes().all(|byte| byte.is_ascii_digit())
        {
            if part
                .strip_prefix("Object file: ")
                .is_some_and(|path| path.replace('\\', "/") == target)
            {
                callers.insert(pid.to_owned());
            } else if let Some(result) = part.strip_prefix("Result: ") {
                results
                    .entry(pid.to_owned())
                    .or_default()
                    .push(result.to_owned());
            }
        }
        bytes.clear();
    }
    if callers.len() != 1 {
        return Err(io::Error::other(
            "The requested object did not have an unambiguous ccache invocation",
        ));
    }
    Ok(results
        .remove(&callers.pop_first().expect("exactly one caller"))
        .unwrap_or_default())
}

impl Probe {
    fn ninja(&self, args: &[&str], timeout: u64) -> io::Result<Vec<u8>> {
        let mut command = process::command(&self.tools.ninja, &self.environment);
        command
            .args(["-C", "out/Numera"])
            .args(args)
            .current_dir(&self.source);
        process::stdout(&mut command, Duration::from_secs(timeout), 64 * 1024 * 1024)
    }

    fn build(
        &self,
        env: &Environment,
        targets: &[&str],
        phase: &str,
        seconds: u64,
    ) -> io::Result<()> {
        let mut command = process::command(&self.tools.ninja, env);
        command
            .args(["-C", "out/Numera", "-j", "4"])
            .args(targets)
            .current_dir(&self.source);
        let path = self.output.join(format!("{phase}.log"));
        let status =
            process::stream_log(&mut command, &path)?.wait(Duration::from_secs(seconds))?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "{phase} object build failed; inspect {phase}.log"
            )));
        }
        Ok(())
    }

    fn object(&self) -> PathBuf {
        self.source.join("out/Numera").join(self.target)
    }

    fn stats(&self, environment: &Environment) -> io::Result<Value> {
        let command = if self.tools.cache_tool == "sccache" {
            "--show-stats"
        } else {
            "--print-stats"
        };
        let format = if self.tools.cache_tool == "sccache" {
            "--stats-format=json"
        } else {
            "--format=json"
        };
        Ok(serde_json::from_slice(&tools::cache_call(
            &self.tools,
            environment,
            &[command, format],
        )?)?)
    }

    fn run_object(&self, phase: &str) -> io::Result<(String, Vec<String>)> {
        let object = self.object();
        if object.exists() {
            fs::remove_file(&object)?;
        }
        tools::cache_call(&self.tools, &self.environment, &["--zero-stats"])?;
        let stats_path = self.output.join(format!("{phase}-statistics.json"));
        let mut env = self.environment.clone();
        let log = self.directory.join(format!("{phase}-ccache.log"));
        if self.tools.cache_tool == "ccache" {
            env.insert("CCACHE_LOGFILE".into(), log.clone().into());
        }
        self.build(&env, &[self.target], phase, 600)?;
        let mut stats = self.stats(&env)?;
        let results = if self.tools.cache_tool == "sccache" {
            let stats = &stats["stats"];
            report::write_json(&stats_path, stats)?;
            if stats["compile_requests"] != 1 {
                return Err(io::Error::other(
                    "The Windows probe did not isolate exactly one compile request",
                ));
            }
            let mut results = Vec::new();
            if tools::sccache_count(stats, "cache_hits") == 1 {
                results.extend(["preprocessed_cache_hit".into(), "local_storage_hit".into()]);
            }
            if tools::sccache_count(stats, "cache_misses") == 1 {
                results.push("cache_miss".into());
            }
            if stats["cache_writes"] == 1 {
                results.push("local_storage_write".into());
            }
            results
        } else {
            let results = ccache_results(&log, self.target);
            if let Ok(results) = &results {
                stats["target_results"] = json!(results);
            }
            report::write_json(&stats_path, &stats)?;
            let results = results?;
            fs::remove_file(log)?;
            results
        };
        Ok((cache::sha256(&object)?, results))
    }

    fn prepare_windows(&self) -> io::Result<()> {
        let response = String::from_utf8(self.ninja(&["-t", "query", self.target], 30)?)
            .map_err(io::Error::other)?;
        let mut lines = response.lines();
        if lines.next() != Some(format!("{}:", self.target).as_str())
            || !lines
                .next()
                .is_some_and(|line| line.starts_with("  input: "))
        {
            return Err(io::Error::other(
                "Unexpected Ninja dependency description for the probe object",
            ));
        }
        let mut inputs = Vec::new();
        for input in lines {
            if input == "  outputs:" {
                break;
            }
            let input = input.trim().trim_start_matches('|').trim();
            if input.is_empty() || input.starts_with('-') || inputs.len() == 128 {
                return Err(io::Error::other(
                    "Unexpected direct dependencies for the probe object",
                ));
            }
            inputs.push(input);
        }
        if inputs.is_empty() || !response.lines().any(|line| line == "  outputs:") {
            return Err(io::Error::other(
                "Unexpected direct dependencies for the probe object",
            ));
        }
        let mut env = self.environment.clone();
        env.insert(
            "SCCACHE_DIR".into(),
            self.directory.join("prerequisite-entries").into(),
        );
        env.insert("SCCACHE_CACHE_SIZE".into(), "128M".into());
        env.insert("SCCACHE_SERVER_PORT".into(), "4233".into());
        tools::cache_runtime(&self.tools, &env, || {
            let built = self.build(&env, &inputs, "object-prerequisites", 24 * 60);
            let stats = self.stats(&env).and_then(|stats| {
                report::write_json(
                    &self.output.join("object-prerequisites-statistics.json"),
                    &stats,
                )
            });
            if stats.is_err() {
                report::log(
                    "WARNING",
                    "compiler-cache-probe",
                    "Could not record prerequisite cache statistics",
                );
            }
            built
        })?;
        if self.object().exists() {
            return Err(io::Error::other(
                "Preparing the Windows inputs unexpectedly built the target object",
            ));
        }
        Ok(())
    }

    fn context(&self, root: &Path, host: Host, config: &Path) -> io::Result<Value> {
        let lines = self.ninja(&["-t", "commands", self.target], 30)?;
        let last = lines
            .split(|byte| *byte == b'\n')
            .rfind(|line| !line.is_empty())
            .ok_or_else(|| io::Error::other("Missing native Ninja compile command"))?;
        let last = last.strip_suffix(b"\r").unwrap_or(last);
        let marker = format!("{} ", self.tools.cache_tool);
        let present = last
            .windows(marker.len())
            .enumerate()
            .any(|(index, bytes)| {
                bytes == marker.as_bytes()
                    && (index == 0
                        || !(last[index - 1].is_ascii_alphanumeric() || last[index - 1] == b'_'))
            });
        if !present {
            return Err(io::Error::other(
                "The actual GN compile command did not use the pinned local compiler wrapper",
            ));
        }
        let clang = self
            .source
            .join("third_party/llvm-build/Release+Asserts/bin")
            .join(if host == Host::Windows {
                "clang-cl.exe"
            } else {
                "clang"
            });
        let lock = Path::new("upstream/chromium/lock.json");
        let mut context = json!({"run_id":getenv("GITHUB_RUN_ID"),"run_attempt":getenv("GITHUB_RUN_ATTEMPT"),
            "runner_image":getenv("ImageVersion"),"head":getenv("GITHUB_SHA"),"host":host.name(),"scratch":root,
            "target":self.target,"lock_sha256":cache::sha256(lock)?,"gn_args_sha256":cache::sha256(&self.source.join("out/Numera/args.gn"))?,
            "cache_tool":self.tools.cache_tool,"cache_tool_sha256":cache::sha256(&self.tools.cache)?,"cache_config_sha256":cache::sha256(config)?,
            "ninja_sha256":cache::sha256(&self.tools.ninja)?,"clang_sha256":cache::sha256(&clang)?,"command_sha256":sha256_hex(last),
            "input_sha256":cache::sha256(&self.source.join("base/logging.cc"))?});
        if host == Host::Windows {
            let raw = fs::read(root.join("windows-sdk.json"))?;
            let sdk: Value =
                serde_json::from_slice(raw.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&raw))?;
            context["sdk"] = json!({"directory_version":sdk["directory_version"],"debugging_tools_version":sdk["debugging_tools_version"],
                "requested_release":sdk["requested_release"]});
        }
        Ok(context)
    }

    fn replay(
        &self,
        seed: bool,
        transfer: &Path,
        context: &Value,
        evidence: &mut Value,
    ) -> io::Result<String> {
        let mut seed_hash = None;
        if !seed {
            if self.object().exists() {
                return Err(io::Error::other(
                    "Fresh runner unexpectedly contains the compiled probe object",
                ));
            }
            let expected = getenv("EXPECTED_CHECKPOINT_SHA");
            let mut options = Restore::new(context, &expected, PROBE_LIMITS);
            options.allow_linux_runner_image = true;
            let restored =
                cache::restore_cache(&self.entries, transfer, &options, &mut Inventory::new())?;
            seed_hash = restored.object_sha256;
            evidence["runner_images"] = restored.runner_images.unwrap_or_default();
        }
        tools::cache_runtime(&self.tools, &self.environment, || {
            let (digest, checks) = self.run_object(if seed { "cold" } else { "restored" })?;
            let has = |value: &str| checks.iter().any(|entry| entry == value);
            if seed {
                if !has("cache_miss") || !has("local_storage_write") {
                    return Err(io::Error::other(
                        "Cold Chromium compilation did not populate an initially empty cache",
                    ));
                }
            } else {
                if !has("preprocessed_cache_hit")
                    || !has("local_storage_hit")
                    || seed_hash.as_deref() != Some(&digest)
                {
                    return Err(io::Error::other(
                        "Different runner did not reuse the seed object",
                    ));
                }
                fs::OpenOptions::new().append(true).open(self.source.join("base/logging.cc"))?.write_all(
                    b"\nextern \"C\" int NumeraCacheProbeValue();\nextern \"C\" int NumeraCacheProbeValue() { return 17; }\n")?;
                let (changed, checks) = self.run_object("changed-source")?;
                if !checks.iter().any(|value| value == "cache_miss")
                    || checks.iter().any(|value| value == "preprocessed_cache_hit")
                    || changed == digest
                {
                    return Err(io::Error::other(
                        "A changed Chromium input failed to invalidate the cached output",
                    ));
                }
                evidence["changed_object_sha256"] = json!(changed);
                evidence["restored_cache_hits"] = json!(1);
                evidence["changed_source_cache_misses"] = json!(1);
            }
            Ok(digest)
        })
    }
}

fn run(phase: &str) -> io::Result<()> {
    let host = report::validate_runner(true)?;
    let project = std::env::current_dir()?;
    fs::create_dir_all(project.join("build"))?;
    let output = project.join("build/chromium-cache-evidence");
    fs::create_dir(&output)?;
    let transfer = project.join("build/chromium-cache-transfer/seed.tar");
    let root = report::required_path("RUNNER_TEMP")?.join("numera-compiler-cache-probe");
    let directory = root.with_file_name("numera-compiler-cache-probe-tools");
    fs::create_dir(&directory)?;
    let entries = directory.join("entries");
    fs::create_dir(&entries)?;
    let mut evidence = json!({"phase":phase,"runner":host.name(),"source_commit":getenv("GITHUB_SHA"),
        "result":"BLOCKED","engine_source_acceptance":"NOT_RUN","release_ready":false});
    let outcome = (|| -> io::Result<()> {
        let mut environment = report::engine_env(&directory, &root)?;
        let tools = tools::install_tools(&directory, host, &environment)?;
        let config = tools::configure_cache(&mut environment, &tools, &directory, &entries, false)?;
        tools::compile_graph(&root, &environment, &output, tools.cache_tool, None)?;
        let target = if host == Host::Windows {
            "obj/base/base/logging.obj"
        } else {
            "obj/base/base/logging.o"
        };
        let probe = Probe {
            tools,
            environment,
            source: root.join("checkout/src"),
            directory,
            entries,
            output: output.clone(),
            target,
        };
        let context = probe.context(&root, host, &config)?;
        evidence["context"] = context.clone();
        if host == Host::Windows {
            probe.prepare_windows()?;
        }
        let seed = phase == "seed";
        let digest = probe.replay(seed, &transfer, &context, &mut evidence)?;
        evidence["object_sha256"] = json!(digest);
        if seed {
            fs::create_dir(transfer.parent().expect("transfer has a parent"))?;
            cache::pack_cache(
                &probe.entries,
                &transfer,
                &context,
                Some(digest),
                PROBE_LIMITS,
                None,
                None,
            )?;
            let sha = cache::sha256(&transfer)?;
            evidence["checkpoint_bytes"] = json!(transfer.metadata()?.len());
            evidence["checkpoint_sha256"] = json!(sha);
            report::github_output(&[("checkpoint_sha", &sha)], true)?;
        }
        evidence["result"] = json!("PASS");
        report::log(
            "INFO",
            "compiler-cache-probe",
            &format!("{phase} passed for a real Chromium object"),
        );
        Ok(())
    })();
    if let Err(error) = &outcome {
        evidence["error_type"] = json!("NativeCommandError");
        evidence["diagnostic"] = json!(error.to_string());
    }
    report::write_json(&output.join("report.json"), &evidence)?;
    outcome
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [phase] if matches!(phase.as_str(), "seed" | "resume") => match run(phase) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                report::log("ERROR", "compiler-cache-probe", &error.to_string());
                ExitCode::FAILURE
            }
        },
        [help] if matches!(help.as_str(), "-h" | "--help") => {
            println!("Usage: numera-chromium-cache-probe <seed|resume>");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("Usage: numera-chromium-cache-probe <seed|resume>");
            ExitCode::from(2)
        }
    }
}
