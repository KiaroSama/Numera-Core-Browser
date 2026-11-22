use core_contracts::{
    BuildPolicy, CoreError, DeviceMode, EngineFamily, ErrorCode, Identity, LaunchSnapshot,
    Lifetime, MAX_FRAME, ProxyPolicy, ResourceGrants, Response, Theme, ThemeMode, read_frame,
    strict_json, valid_id,
};
use core_engines::EngineStore;
use core_runtime::{Connection, Sessions};
use serde_json::{Value, json};
use std::{
    env,
    io::{self, BufReader, Read, Write},
    path::Path,
    process::ExitCode,
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn terminal_log(level: &str, message: &str) {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let record = json!({"timestamp_unix_ms":timestamp,"timezone":"UTC","severity":level,"component":"core-cli","message":message}).to_string();
    let (send, receive) = mpsc::sync_channel(1);
    // One terminal event only: an unread inherited stderr pipe must not defeat
    // protocol cancellation. Never join the worker or reacquire its I/O lock.
    // Process exit ends a stalled worker; diagnostics are best effort.
    if thread::Builder::new()
        .name("terminal-diagnostic".into())
        .spawn(move || {
            let mut stderr = io::stderr().lock();
            let _ = writeln!(stderr, "{record}").and_then(|()| stderr.flush());
            let _ = send.send(());
        })
        .is_ok()
    {
        let _ = receive.recv_timeout(Duration::from_millis(100));
    }
}
fn output(value: &Value) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, value)?;
    stdout.write_all(b"\n")?;
    stdout.flush()
}
fn read_bounded(path: &str) -> Result<Vec<u8>, CoreError> {
    let mut bytes = Vec::new();
    core_engines::open_regular_file(Path::new(path))?
        .take(MAX_FRAME as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| CoreError::invalid("Cannot read selected input file"))?;
    if bytes.len() >= MAX_FRAME {
        return Err(CoreError::invalid("Input file exceeds limit"));
    }
    Ok(bytes)
}

type PendingOutput = (Vec<u8>, mpsc::SyncSender<io::Result<()>>);
struct ProtocolOutput(mpsc::SyncSender<PendingOutput>);
impl ProtocolOutput {
    fn new() -> Self {
        let (send, receive) = mpsc::sync_channel::<PendingOutput>(1);
        thread::spawn(move || {
            let mut stdout = io::stdout().lock();
            while let Ok((bytes, reply)) = receive.recv() {
                let result = stdout.write_all(&bytes).and_then(|()| stdout.flush());
                let failed = result.is_err();
                let _ = reply.send(result);
                if failed {
                    break;
                }
            }
        });
        Self(send)
    }
    fn write(&self, response: Response, budget: Duration) -> Result<(), CoreError> {
        let mut bytes = serde_json::to_vec(&response)
            .map_err(|_| CoreError::invalid("Cannot encode response"))?;
        bytes.push(b'\n');
        if bytes.len() > MAX_FRAME {
            return Err(CoreError::new(
                ErrorCode::InsufficientResources,
                "Response exceeds frame limit",
            ));
        }
        let (reply, receive) = mpsc::sync_channel(1);
        let closed = || {
            CoreError::new(
                ErrorCode::Cancellation,
                "Protocol output closed or backpressure timeout",
            )
        };
        self.0.send((bytes, reply)).map_err(|_| closed())?;
        receive
            .recv_timeout(budget.min(Duration::from_secs(5)))
            .map_err(|_| closed())?
            .map_err(|_| closed())
    }
    fn terminal(&self, error: CoreError) -> CoreError {
        let _ = self.write(
            Response::new(None, Err(error.clone())),
            Duration::from_secs(5),
        );
        error
    }
}
fn serve(mut connection: Connection) -> Result<(), CoreError> {
    // A single bounded slot propagates output backpressure to input. A timed
    // receive bounds partial-frame/idle stalls. Connection drop closes worker pipes.
    let (send, receive) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut reader = BufReader::new(io::stdin());
        loop {
            let frame = read_frame(&mut reader);
            let ended = !matches!(&frame, Ok(Some(_)));
            if send.send(frame).is_err() || ended {
                break;
            }
        }
    });
    let output = ProtocolOutput::new();
    loop {
        let frame = receive.recv_timeout(Duration::from_secs(60)).map_err(|_| {
            output.terminal(CoreError::new(
                ErrorCode::Cancellation,
                "Input idle or partial-frame timeout",
            ))
        })?;
        let bytes = match frame {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Ok(()),
            Err(_) => {
                return Err(
                    output.terminal(CoreError::invalid("Invalid or incomplete NDJSON frame"))
                );
            }
        };
        let received = Instant::now();
        let budget = core_contracts::Request::decode(&bytes).map_or(Duration::from_secs(5), |r| {
            Duration::from_millis(r.deadline_ms as u64)
        });
        let response = connection.handle(&bytes);
        output.write(response, budget.saturating_sub(received.elapsed()))?;
    }
}

fn read_snapshot(path: &str) -> Result<LaunchSnapshot, CoreError> {
    serde_json::from_value(strict_json(&read_bounded(path)?)?)
        .map_err(|_| CoreError::invalid("Invalid launch snapshot"))
}

fn sessions(store: &str, data: &str) -> Result<Sessions, CoreError> {
    let worker = env::current_exe()
        .map_err(|_| CoreError::invalid("Cannot locate native Core executable"))?;
    Sessions::new(Path::new(store), Path::new(data), &worker)
}

fn package_json<T: serde::Serialize>(result: T) -> Result<Value, CoreError> {
    serde_json::to_value(result).map_err(|_| CoreError::invalid("Cannot encode package report"))
}

fn command(args: &[String]) -> Result<Value, CoreError> {
    let strings: Vec<&str> = args.iter().map(String::as_str).collect();
    match strings.as_slice() {
        ["doctor"] | ["diagnostics"] => Ok(core_runtime::doctor()),
        ["capabilities"] => Ok(core_runtime::capabilities()),
        ["engines", "list"] => Ok(core_engines::inventory()),
        ["engines", "list", store] => EngineStore::open(Path::new(store))?.inventory(),
        ["engines", "verify", store, digest] => {
            package_json(EngineStore::open(Path::new(store))?.verify(digest)?)
        }
        ["engines", "install", store, root, repository] => package_json(
            EngineStore::open(Path::new(store))?.install(Path::new(root), Path::new(repository))?,
        ),
        ["engines", "target", archive, manifest_path] => {
            let manifest = core_engines::inspect_manifest(&read_bounded(manifest_path)?)?;
            let path = Path::new(archive);
            let digest = core_engines::hash_file(path)?;
            package_json(core_engines::describe_runtime_archive(
                path, &digest, &manifest,
            )?)
        }
        ["updates", "check", store, root, repository] => package_json(
            EngineStore::open(Path::new(store))?.check(Path::new(root), Path::new(repository))?,
        ),
        ["updates", "stage", store, root, repository] => package_json(
            EngineStore::open(Path::new(store))?.stage(Path::new(root), Path::new(repository))?,
        ),
        ["updates", "activate", store, digest] => {
            package_json(EngineStore::open(Path::new(store))?.activate(digest)?)
        }
        ["updates", "rollback", store] => {
            package_json(EngineStore::open(Path::new(store))?.rollback()?)
        }
        ["profiles", "snapshot", data, source, destination] => {
            let digest = core_process::snapshot_profile(Path::new(data), source, destination)?;
            Ok(json!({"snapshot_complete":true,"package_sha256":digest}))
        }
        ["engines", "verify-archive", path, sha256] => {
            let report = core_engines::inspect_runtime_archive(Path::new(path), sha256)?;
            Ok(
                json!({"archive_verified":true,"archive_sha256":report.sha256,"archive_bytes":report.archive_bytes,
                "files":report.files,"expanded_file_bytes":report.file_bytes,"approved_package":false}),
            )
        }
        ["plan", path] => core_runtime::plan(&read_snapshot(path)?),
        ["plan", store, data, path] => sessions(store, data)?.plan(&read_snapshot(path)?),
        ["launch", store, data, path] => {
            sessions(store, data)?.launch_foreground(read_snapshot(path)?)?;
            Ok(json!({"state":"stopped"}))
        }
        ["manifest", "inspect", path] => {
            let m = core_engines::inspect_manifest(&read_bounded(path)?)?;
            Ok(
                json!({"manifest_valid":true,"family":m.family,"package_id":m.package_id,
                "trusted":false,"installable":false,"reason":"Structural validation is not signature verification or release approval"}),
            )
        }
        ["hash-file", path] => {
            Ok(json!({"sha256":core_engines::hash_file(Path::new(path))?,"trusted":false}))
        }
        ["schema", "request"] => Ok(core_contracts::request_schema()),
        ["schema", "response"] => {
            Ok(serde_json::to_value(schemars::schema_for!(Response)).expect("Schema serializes"))
        }
        ["schema", "manifest"] => Ok(serde_json::to_value(schemars::schema_for!(
            core_engines::EngineManifest
        ))
        .expect("Schema serializes")),
        ["--version"] | ["version"] => {
            Ok(json!({"product":"Numera Core Browser","version":env!("CARGO_PKG_VERSION")}))
        }
        [] | ["help"] | ["--help"] => Ok(
            json!({"usage":["numera-core [--json] doctor","numera-core capabilities","numera-core engines list [STORE]","numera-core engines verify-archive FILE SHA256","numera-core engines target ARCHIVE MANIFEST.json","numera-core engines verify STORE SHA256","numera-core engines install STORE TRUSTED_ROOT REPOSITORY","numera-core updates check|stage STORE TRUSTED_ROOT REPOSITORY","numera-core updates activate STORE SHA256","numera-core updates rollback STORE","numera-core profiles snapshot DATA_ROOT SOURCE DESTINATION","numera-core plan [STORE DATA_ROOT] SNAPSHOT.json","numera-core launch STORE DATA_ROOT SNAPSHOT.json","numera-core serve [STORE DATA_ROOT]","numera-core schema request|response|manifest","numera-core manifest inspect MANIFEST.json","numera-core hash-file FILE"],
            "status":"Development control plane. Native sessions require a publisher-approved package in an explicit store."}),
        ),
        _ => Err(CoreError::invalid(
            "Unknown command or arguments; use --help",
        )),
    }
}

fn desktop(args: &[String]) -> Result<Value, CoreError> {
    if args.is_empty() || args == ["--help"] {
        return Ok(json!({"usage":"numera-core-gui STORE DATA_ROOT PROFILE [URL]"}));
    }
    let [store, data, profile, rest @ ..] = args else {
        return Err(CoreError::invalid(
            "Use numera-core-gui STORE DATA_ROOT PROFILE [URL]",
        ));
    };
    if !valid_id(profile) || matches!(profile.as_str(), "." | "..") || rest.len() > 1 {
        return Err(CoreError::invalid(
            "Invalid desktop profile identifier or startup arguments",
        ));
    }
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|_| {
        CoreError::new(
            ErrorCode::InsufficientResources,
            "Cannot obtain a launch identifier",
        )
    })?;
    let random: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let selected = Path::new(data)
        .join("profiles")
        .join(format!("profile-{profile}"));
    let path = |name| {
        selected
            .join(name)
            .into_os_string()
            .into_string()
            .map_err(|_| CoreError::invalid("Desktop paths must be valid UTF-8"))
    };
    let native_profile_dir = path("browser")?;
    let build_policy = core_process::profile_package(Path::new(data), &native_profile_dir)?
        .map_or(BuildPolicy::Approved, |sha256| BuildPolicy::Pinned {
            sha256,
        });
    let snapshot = LaunchSnapshot {
        profile_id: profile.clone(),
        launch_id: random.clone(),
        idempotency_key: random.clone(),
        engine: EngineFamily::Chromium,
        build_policy,
        device_mode: DeviceMode::Desktop,
        required_capabilities: Vec::new(),
        fingerprint_seed: random,
        identity: Identity {
            viewport_width: 1280,
            viewport_height: 720,
            screen_width: 1920,
            screen_height: 1080,
            device_scale_factor: 1.0,
            touch: false,
            language: "en-US".into(),
            timezone: "Etc/UTC".into(),
        },
        startup_urls: rest.to_vec(),
        resources: ResourceGrants {
            native_profile_dir,
            data_root: data.clone(),
            cache_root: path("cache")?,
            ownership_grant_ref: None,
        },
        extensions: Vec::new(),
        lifetime: Lifetime::Standalone,
        proxy: ProxyPolicy::Direct,
        theme: Theme {
            revision: 1,
            mode: ThemeMode::System,
        },
        accept_emulation_limitations: false,
    };
    sessions(store, data)?.launch_foreground(snapshot)?;
    Ok(json!({"state":"stopped"}))
}

pub fn run(gui: bool) -> ExitCode {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--json") {
        args.remove(0);
    }
    let worker = args.as_slice() == ["__session-worker"];
    let server = !gui && args.first().is_some_and(|arg| arg == "serve");
    let result = if worker {
        core_runtime::run_worker()
    } else if server {
        match args.as_slice() {
            [_] => serve(Connection::default()),
            [_, store, data] => sessions(store, data)
                .and_then(|sessions| serve(Connection::with_sessions(sessions))),
            _ => Err(CoreError::invalid(
                "Use numera-core serve [STORE DATA_ROOT]",
            )),
        }
    } else {
        (if gui { desktop(&args) } else { command(&args) }).and_then(|v| {
            output(&v).map_err(|_| CoreError::new(ErrorCode::Cancellation, "Output closed"))
        })
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            terminal_log("ERROR", &error.message);
            if !server && !worker {
                let _ = output(
                    &serde_json::to_value(Response::new(None, Err(error.clone())))
                        .expect("Error serializes"),
                );
            }
            ExitCode::from(match error.code {
                ErrorCode::MissingPackage => 3,
                ErrorCode::Cancellation => 4,
                _ => 2,
            })
        }
    }
}
