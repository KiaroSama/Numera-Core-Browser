use core_contracts::{
    CoreError, DeviceMode, ErrorCode, LaunchSnapshot, Lifetime, ProxyPolicy, ThemeMode, read_frame,
    strict_json, valid_id,
};
use core_engines::{EngineStore, PackageInfo, Support};
use core_process::{BrowserProcess, SessionPaths};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const STOP_BUDGET: Duration = Duration::from_secs(5);
const MAX_SESSIONS: usize = 16;
const MAX_RECORDS: usize = 64;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerRequest {
    store: PathBuf,
    root: PathBuf,
    snapshot: LaunchSnapshot,
    package: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
enum State {
    Running,
    Stopped,
    RecoveryRequired,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SessionView {
    launch_id: String,
    profile_id: String,
    package_sha256: String,
    device_mode: DeviceMode,
    state: State,
    sequence: u64,
    browser_process_started: bool,
    automation_endpoints: Vec<String>,
}

fn failure(message: &'static str) -> CoreError {
    CoreError::new(ErrorCode::LaunchFailure, message)
}

fn cancelled() -> CoreError {
    CoreError::new(ErrorCode::Cancellation, "Browser request deadline exceeded")
}

fn separate_roots(store: &Path, root: &Path) -> Result<(), CoreError> {
    let resolved_store = store.canonicalize().unwrap_or_else(|_| store.to_owned());
    let resolved_root = root.canonicalize().unwrap_or_else(|_| root.to_owned());
    if resolved_store.starts_with(&resolved_root) || resolved_root.starts_with(&resolved_store) {
        return Err(CoreError::invalid(
            "Installed packages and writable profiles require separate roots",
        ));
    }
    Ok(())
}

fn read<T: DeserializeOwned>(input: &mut impl BufRead) -> Result<T, CoreError> {
    let bytes = read_frame(input)
        .map_err(|_| failure("Invalid native worker frame"))?
        .ok_or_else(|| failure("Native worker disconnected"))?;
    serde_json::from_value(strict_json(&bytes)?)
        .map_err(|_| failure("Unexpected native worker response"))
}

fn write(out: &mut impl Write, data: &impl Serialize) -> Result<(), CoreError> {
    serde_json::to_writer(&mut *out, data)
        .map_err(|_| failure("Cannot send native worker frame"))?;
    out.write_all(b"\n")
        .and_then(|()| out.flush())
        .map_err(|_| failure("Native worker channel closed"))
}

pub(crate) fn supplied_features() -> &'static [&'static str] {
    &[
        "protocol.ndjson",
        "launch.plan",
        "session.launch",
        "browser.navigation",
        "browser.gui",
    ]
}

pub(crate) fn choose(
    store: &EngineStore,
    root: &Path,
    snapshot: &LaunchSnapshot,
) -> Result<PackageInfo, CoreError> {
    let unsupported = |message| CoreError::new(ErrorCode::UnsupportedCapability, message);
    SessionPaths::validate(root, snapshot)?;
    if !matches!(snapshot.lifetime, Lifetime::Standalone) {
        return Err(unsupported(
            "This session authority operates in standalone mode. The manager grant and detached supervisor are not configured",
        ));
    }
    if snapshot.device_mode != DeviceMode::Desktop || snapshot.identity.touch {
        return Err(unsupported(
            "This native session does not apply mobile or touch emulation",
        ));
    }
    if !snapshot.extensions.is_empty() {
        return Err(unsupported(
            "Extension grants require an installed and authenticated extension provider",
        ));
    }
    if !matches!(snapshot.theme.mode, ThemeMode::System) {
        return Err(unsupported(
            "Explicit browser chrome theming is unavailable for this package. Use the system theme",
        ));
    }
    network_arguments(&snapshot.proxy)?;
    let selected = store.select(&snapshot.engine, &snapshot.build_policy)?;
    if snapshot.required_capabilities.iter().any(|name| {
        !supplied_features().contains(&name.as_str())
            || (!matches!(
                name.as_str(),
                "protocol.ndjson" | "launch.plan" | "session.launch"
            ) && !selected.manifest.features.iter().any(|feature| {
                feature.name == *name && matches!(feature.status, Support::Supported)
            }))
    }) {
        return Err(unsupported(
            "A required capability is unavailable for the selected browser",
        ));
    }
    Ok(selected)
}

fn child_environment(paths: Option<&SessionPaths>) -> BTreeMap<String, OsString> {
    let names = [
        "HOME",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XAUTHORITY",
        "XDG_RUNTIME_DIR",
        "XDG_SESSION_TYPE",
        "XDG_CURRENT_DESKTOP",
        "DBUS_SESSION_BUS_ADDRESS",
        "SystemRoot",
        "WINDIR",
        "SYSTEMDRIVE",
        "USERPROFILE",
        "LOCALAPPDATA",
        "APPDATA",
        "PROGRAMDATA",
    ];
    let mut environment: BTreeMap<_, _> = names
        .into_iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (name.to_owned(), value)))
        .collect();
    if let Some(paths) = paths {
        for name in ["TMP", "TEMP", "TMPDIR"] {
            environment.insert(name.into(), paths.cache().as_os_str().into());
        }
    }
    environment
}

fn network_arguments(proxy: &ProxyPolicy) -> Result<Vec<OsString>, CoreError> {
    let ProxyPolicy::Required {
        endpoint,
        secret_ref,
    } = proxy
    else {
        return Ok(vec!["--no-proxy-server".into()]);
    };
    if secret_ref.is_some() {
        return Err(CoreError::new(
            ErrorCode::ProxyFailure,
            "This session has no protected proxy credential provider",
        ));
    }
    let url =
        url::Url::parse(endpoint).map_err(|_| CoreError::invalid("Invalid proxy endpoint"))?;
    let scheme = match url.scheme() {
        "http" => "http",
        "https" => "https",
        "socks5" | "socks5h" => "socks5",
        _ => return Err(CoreError::invalid("Unsupported proxy scheme")),
    };
    let host = match url.host() {
        Some(url::Host::Ipv4(host)) => host.to_string(),
        Some(url::Host::Ipv6(host)) => format!("[{host}]"),
        Some(url::Host::Domain(host))
            if host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')) =>
        {
            host.to_owned()
        }
        _ => return Err(CoreError::invalid("Unsupported proxy host syntax")),
    };
    let port = url
        .port_or_known_default()
        .filter(|port| *port != 0)
        .ok_or_else(|| CoreError::invalid("Expected a nonzero proxy port"))?;
    Ok(vec![
        format!("--proxy-server={scheme}://{host}:{port}").into(),
        "--proxy-bypass-list=<-loopback>".into(),
        format!("--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE {host}").into(),
        "--disable-quic".into(),
        "--force-webrtc-ip-handling-policy=disable_non_proxied_udp".into(),
    ])
}

fn browser_arguments(
    snapshot: &LaunchSnapshot,
    paths: &SessionPaths,
    network: Vec<OsString>,
) -> Vec<OsString> {
    fn path_flag(name: &str, path: &Path) -> OsString {
        let mut flag = OsString::from(name);
        flag.push(path);
        flag
    }
    let identity = &snapshot.identity;
    let mut arguments = vec![
        path_flag("--user-data-dir=", paths.profile()),
        path_flag("--disk-cache-dir=", paths.cache()),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        format!("--lang={}", identity.language).into(),
        format!(
            "--window-size={},{}",
            identity.viewport_width, identity.viewport_height
        )
        .into(),
    ];
    arguments.extend(network);
    arguments.extend(snapshot.startup_urls.iter().map(OsString::from));
    arguments
}

/// Internal executable entry point. The first frame only requests validation.
/// An additional byte from the same inherited pipe authorizes process creation.
pub fn run_worker() -> Result<(), CoreError> {
    let mut input = BufReader::new(io::stdin());
    let mut output = io::stdout();
    let prepared = (|| {
        let mut request: WorkerRequest = read(&mut input)?;
        crate::plan(&request.snapshot)?;
        separate_roots(&request.store, &request.root)?;
        let store = EngineStore::open(&request.store)?;
        request.snapshot.build_policy = core_contracts::BuildPolicy::Pinned {
            sha256: request.package.clone(),
        };
        choose(&store, &request.root, &request.snapshot)?;
        let engine = store.resolve(&request.snapshot.engine, &request.snapshot.build_policy)?;
        if engine.sha256() != request.package {
            return Err(failure("Native worker selected a different package"));
        }
        let paths = SessionPaths::validate(&request.root, &request.snapshot)?;
        let network = network_arguments(&request.snapshot.proxy)?;
        Ok((request, engine, paths, network))
    })();
    let (request, engine, paths, network) = match prepared {
        Ok(prepared) => {
            write(&mut output, &Result::<(), CoreError>::Ok(()))?;
            prepared
        }
        Err(error) => {
            write(&mut output, &Result::<(), CoreError>::Err(error))?;
            return Ok(());
        }
    };
    let mut start = [0];
    if input.read_exact(&mut start).is_err() || start != [1] {
        return Ok(());
    }
    let snapshot = &request.snapshot;
    let process = BrowserProcess::start(
        paths,
        snapshot,
        engine.sha256(),
        engine.executable(),
        |paths| {
            (
                browser_arguments(snapshot, paths, network),
                child_environment(Some(paths)),
            )
        },
    );
    let process = match process {
        Ok(process) => process,
        Err(error) => {
            write(&mut output, &Result::<SessionView, CoreError>::Err(error))?;
            return Ok(());
        }
    };
    let (finished, control) = mpsc::sync_channel(1);
    let watcher = thread::Builder::new()
        .name("session-parent".into())
        .spawn(move || {
            let _ = input.read(&mut [0]);
            let _ = finished.send(());
        });
    if watcher.is_err() {
        process.stop(Instant::now() + STOP_BUDGET)?;
        return Err(failure("Cannot watch session owner"));
    }
    let run = (|| {
        if control.recv_timeout(Duration::from_millis(200)).is_ok() {
            return Err(cancelled());
        }
        if process.root_exited()? {
            return Err(failure("Browser exited during startup"));
        }
        write(
            &mut output,
            &Ok::<_, CoreError>(SessionView {
                launch_id: snapshot.launch_id.clone(),
                profile_id: snapshot.profile_id.clone(),
                package_sha256: request.package,
                device_mode: snapshot.device_mode.clone(),
                state: State::Running,
                sequence: 1,
                browser_process_started: true,
                automation_endpoints: Vec::new(),
            }),
        )?;
        while !process.root_exited()? {
            match control.recv_timeout(Duration::from_millis(100)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => (),
            }
        }
        Ok(())
    })();
    let stopped = process.stop(Instant::now() + STOP_BUDGET);
    stopped.and(run)
}

struct Worker {
    child: Option<Child>,
    control: Option<ChildStdin>,
}

fn receive<T>(
    channel: &mpsc::Receiver<Result<T, CoreError>>,
    deadline: Instant,
) -> Result<T, CoreError> {
    channel
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => cancelled(),
            mpsc::RecvTimeoutError::Disconnected => failure("Native worker disconnected"),
        })?
}

impl Worker {
    fn start(
        executable: &Path,
        spec: WorkerRequest,
        deadline: Instant,
    ) -> Result<(Self, SessionView), CoreError> {
        let mut command = Command::new(executable);
        command
            .arg("__session-worker")
            .env_clear()
            .envs(child_environment(None))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut command, 0x0000_0200);
        let mut child = command
            .spawn()
            .map_err(|_| failure("Cannot launch the native session worker"))?;
        let mut input = child.stdin.take().expect("piped input");
        let mut output = BufReader::new(child.stdout.take().expect("piped output"));
        let mut worker = Self {
            child: Some(child),
            control: None,
        };
        let (ready_out, ready_in) = mpsc::sync_channel(1);
        let (go, waiting) = mpsc::sync_channel::<()>(1);
        let (start_out, start_in) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("worker-startup".into())
            .spawn(move || {
                let ready = write(&mut input, &spec)
                    .and_then(|()| read::<Result<(), CoreError>>(&mut output)?);
                let accepted = ready.is_ok();
                if ready_out.send(ready).is_err() || !accepted || waiting.recv().is_err() {
                    return;
                }
                let response = input
                    .write_all(&[1])
                    .and_then(|()| input.flush())
                    .map_err(|_| failure("Worker command pipe closed"))
                    .and_then(|()| read::<Result<SessionView, CoreError>>(&mut output)?)
                    .map(|view| (view, input));
                let _ = start_out.send(response);
            })
            .map_err(|_| failure("Cannot start worker channel"))?;
        receive(&ready_in, deadline)?;
        if Instant::now() >= deadline {
            return Err(cancelled());
        }
        go.send(())
            .map_err(|_| failure("Worker command pipe closed"))?;
        let (view, control) = receive(&start_in, deadline)?;
        worker.control = Some(control);
        Ok((worker, view))
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.control.take();
        if let Some(mut child) = self.child.take() {
            // The child's EOF contract owns browser cleanup; forcibly killing
            // the Linux supervisor could strand native descendants.
            let _ = thread::Builder::new()
                .name("session-reap".into())
                .spawn(move || child.wait());
        }
    }
}

struct Session {
    request: Vec<u8>,
    idempotency: String,
    initial: SessionView,
    current: SessionView,
    worker: Worker,
}

impl Session {
    fn refresh(&mut self) -> Result<(), CoreError> {
        if let Some(child) = &mut self.worker.child
            && let Some(status) = child
                .try_wait()
                .map_err(|_| failure("Cannot observe the session worker"))?
        {
            self.current.state = if status.success() {
                State::Stopped
            } else {
                State::RecoveryRequired
            };
            self.current.sequence += 1;
            self.worker.child.take();
            self.worker.control.take();
        }
        Ok(())
    }

    fn stop(&mut self, deadline: Instant) -> Result<(), CoreError> {
        self.worker.control.take();
        while self.worker.child.is_some() {
            self.refresh()?;
            if self.worker.child.is_none() {
                break;
            }
            if Instant::now() >= deadline {
                return Err(cancelled());
            }
            thread::sleep(Duration::from_millis(20));
        }
        if matches!(self.current.state, State::RecoveryRequired) {
            return Err(CoreError::new(
                ErrorCode::RecoveryRequired,
                "Cannot prove native session cleanup",
            ));
        }
        Ok(())
    }
}

pub struct Sessions {
    pub(crate) store: EngineStore,
    pub(crate) root: PathBuf,
    store_path: PathBuf,
    executable: PathBuf,
    records: HashMap<String, Session>,
}

impl Sessions {
    pub fn new(store_path: &Path, root: &Path, executable: &Path) -> Result<Self, CoreError> {
        if !store_path.is_absolute() || !root.is_absolute() || !executable.is_absolute() {
            return Err(CoreError::invalid(
                "Configured store, data root and worker require absolute paths",
            ));
        }
        separate_roots(store_path, root)?;
        Ok(Self {
            store: EngineStore::open(store_path)?,
            root: root.to_owned(),
            store_path: store_path.to_owned(),
            executable: executable.to_owned(),
            records: HashMap::new(),
        })
    }

    pub fn plan(&self, snapshot: &LaunchSnapshot) -> Result<Value, CoreError> {
        crate::plan_with(snapshot, Some(self))
    }

    fn record(&mut self, launch_id: &str) -> Result<&mut Session, CoreError> {
        if !valid_id(launch_id) {
            return Err(CoreError::invalid("Invalid session handle"));
        }
        let session = self
            .records
            .get_mut(launch_id)
            .ok_or_else(|| CoreError::invalid("Unknown session handle on this connection"))?;
        session.refresh()?;
        Ok(session)
    }

    pub(crate) fn launch(
        &mut self,
        snapshot: LaunchSnapshot,
        deadline: Instant,
    ) -> Result<Value, CoreError> {
        crate::plan(&snapshot)?;
        let request = serde_json::to_vec(&snapshot)
            .map_err(|_| CoreError::invalid("Invalid session snapshot"))?;
        let launch_id = snapshot.launch_id.clone();
        if let Some(current) = self.records.get_mut(&launch_id) {
            if current.request != request {
                return Err(CoreError::new(
                    ErrorCode::OwnershipConflict,
                    "Launch ID has a different immutable request",
                ));
            }
            current.refresh()?;
            return Ok(json!(current.current));
        }
        if self
            .records
            .values()
            .any(|record| record.idempotency == snapshot.idempotency_key)
        {
            return Err(CoreError::new(
                ErrorCode::OwnershipConflict,
                "Idempotency key is already bound to another launch",
            ));
        }
        for record in self.records.values_mut() {
            record.refresh()?;
        }
        if self.records.len() >= MAX_RECORDS
            || self
                .records
                .values()
                .filter(|record| record.worker.child.is_some())
                .count()
                >= MAX_SESSIONS
        {
            return Err(CoreError::new(
                ErrorCode::InsufficientResources,
                "Session or retained launch limit reached",
            ));
        }
        let package = choose(&self.store, &self.root, &snapshot)?;
        if Instant::now() >= deadline {
            return Err(cancelled());
        }
        let idempotency = snapshot.idempotency_key.clone();
        let (worker, current) = Worker::start(
            &self.executable,
            WorkerRequest {
                store: self.store_path.clone(),
                root: self.root.clone(),
                snapshot,
                package: package.sha256,
            },
            deadline,
        )?;
        let response = json!(current);
        self.records.insert(
            launch_id,
            Session {
                request,
                idempotency,
                initial: current.clone(),
                current,
                worker,
            },
        );
        Ok(response)
    }

    pub(crate) fn status(&mut self, id: &str) -> Result<Value, CoreError> {
        Ok(json!(self.record(id)?.current))
    }

    pub(crate) fn list(&mut self) -> Result<Value, CoreError> {
        for record in self.records.values_mut() {
            record.refresh()?;
        }
        let mut sessions: Vec<_> = self
            .records
            .values()
            .map(|record| &record.current)
            .collect();
        sessions.sort_by_key(|view| &view.launch_id);
        Ok(json!({"sessions": sessions}))
    }

    pub(crate) fn stop(&mut self, id: &str, deadline: Instant) -> Result<Value, CoreError> {
        let record = self.record(id)?;
        record.stop(deadline)?;
        Ok(json!(record.current))
    }

    pub(crate) fn events(&mut self, id: &str, after: u64) -> Result<Value, CoreError> {
        let record = self.record(id)?;
        if after > record.current.sequence {
            return Err(CoreError::invalid(
                "Session event cursor is ahead of the retained sequence",
            ));
        }
        let mut events = Vec::new();
        if after == 0 {
            events.push(&record.initial);
        }
        if record.current.sequence > 1 && after < record.current.sequence {
            events.push(&record.current);
        }
        Ok(json!({"events": events, "next_sequence": record.current.sequence}))
    }

    pub fn launch_foreground(&mut self, snapshot: LaunchSnapshot) -> Result<(), CoreError> {
        let id = snapshot.launch_id.clone();
        self.launch(snapshot, Instant::now() + Duration::from_secs(120))?;
        let record = self.record(&id)?;
        while record.worker.child.is_some() {
            record.refresh()?;
            thread::sleep(Duration::from_millis(100));
        }
        record.stop(Instant::now())
    }
}

impl Drop for Sessions {
    fn drop(&mut self) {
        for record in self.records.values_mut() {
            record.worker.control.take();
        }
        let deadline = Instant::now() + STOP_BUDGET + Duration::from_secs(1);
        for record in self.records.values_mut() {
            let _ = record.stop(deadline);
        }
    }
}
