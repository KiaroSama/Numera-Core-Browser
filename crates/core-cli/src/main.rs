use core_contracts::{
    CoreError, ErrorCode, LaunchSnapshot, MAX_FRAME, Response, read_frame, strict_json,
};
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
fn serve() -> Result<(), CoreError> {
    // A single bounded slot propagates output backpressure to input. A timed
    // receive bounds partial-frame/idle stalls. On EOF no child process exists.
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
    let mut connection = core_runtime::Connection::default();
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

fn command(args: &[String]) -> Result<Value, CoreError> {
    let strings: Vec<&str> = args.iter().map(String::as_str).collect();
    match strings.as_slice() {
        ["doctor"] | ["diagnostics"] => Ok(core_runtime::doctor()),
        ["capabilities"] => Ok(core_runtime::capabilities()),
        ["engines", "list"] => Ok(core_engines::inventory()),
        ["plan", path] => {
            let snapshot: LaunchSnapshot =
                serde_json::from_value(strict_json(&read_bounded(path)?)?)
                    .map_err(|_| CoreError::invalid("Invalid launch snapshot"))?;
            core_runtime::plan(&snapshot)
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
            json!({"usage":["numera-core [--json] doctor","numera-core capabilities","numera-core engines list","numera-core plan SNAPSHOT.json","numera-core serve","numera-core schema request|response|manifest","numera-core manifest inspect MANIFEST.json","numera-core hash-file FILE"],
            "status":"Development control plane; no approved browser engine or GUI available"}),
        ),
        _ => Err(CoreError::invalid(
            "Unknown command or arguments; use --help",
        )),
    }
}

fn main() -> ExitCode {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--json") {
        args.remove(0);
    }
    let server = args.as_slice() == ["serve"];
    let result = if server {
        serve()
    } else {
        command(&args).and_then(|v| {
            output(&v).map_err(|_| CoreError::new(ErrorCode::Cancellation, "Output closed"))
        })
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            terminal_log("ERROR", &error.message);
            if !server {
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
