//! Browser launch planning, native session ownership and the inherited-stdio protocol.
mod session;
pub use session::{Sessions, run_worker};

use core_contracts::{
    BuildPolicy, Command, CoreError, EngineFamily, ErrorCode, LaunchSnapshot, Lifetime, NAMESPACE,
    PROTOCOL, ProxyPolicy, Request, Response, SCHEMA_REVISION, valid_id,
};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::{Component, Path},
    time::{Duration, Instant},
};

pub fn capabilities() -> Value {
    json!({
        "core_version": env!("CARGO_PKG_VERSION"), "namespace": NAMESPACE,
        "protocol": PROTOCOL, "schema_revision": SCHEMA_REVISION,
        "host": {"os": std::env::consts::OS, "arch": std::env::consts::ARCH},
        "scope": {"engine_build": null, "device_mode": null, "automation_transport": null},
        "features": {
            "protocol.ndjson": {"status":"supported","evidence":"C02/contracts-and-process-conformance"},
            "launch.plan": {"status":"partial","limitations":["Validates configuration only; no installed engine or native settings application"],"evidence":"C03/planning"},
            "browser.navigation": {"status":"unsupported","limitations":["No approved owned engine package"]},
            "browser.gui": {"status":"unsupported","limitations":["This CLI cannot start an approved browser GUI"]},
            "session.launch": {"status":"unsupported","limitations":["No approved engine or durable supervisor"]},
            "automation.playwright.cdp": {"status":"unsupported","limitations":["No managed Chromium CDP endpoint"]},
            "automation.playwright.native": {"status":"unsupported","limitations":["No version-paired native transport"]},
            "automation.puppeteer": {"status":"unsupported","limitations":["No managed Chromium CDP endpoint"]},
            "automation.selenium": {"status":"unsupported","limitations":["No approved engine and ChromeDriver package for managed sessions"]},
            "updates.trusted": {"status":"unsupported","limitations":["No signed root, approved release, staging or rollback implementation"]},
            "mobile.emulation": {"status":"unsupported","limitations":["Validation is not engine-applied emulation"]}
        }
    })
}

pub fn doctor() -> Value {
    json!({"product":"Numera Core Browser", "core_version":env!("CARGO_PKG_VERSION"),
        "development_status":"BLOCKED", "owned_engine_available":false,
        "browser_gui_available":false, "manager_integration_ready":false,
        "display_required":false,"node_required":false,"python_required":false,
        "protocol":{"namespace":NAMESPACE,"major":PROTOCOL,"schema_revision":SCHEMA_REVISION},
        "host":{"os":std::env::consts::OS,"arch":std::env::consts::ARCH},
        "engines":core_engines::inventory(),
        "limitations":["No approved browser package is installed", "Native browser launch and managed automation are unavailable in this CLI", "Engine download, installation and updates are unavailable in this CLI"]})
}

fn absolute_lexical(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\0')
        && Path::new(path).is_absolute()
        && !Path::new(path)
            .components()
            .any(|c| matches!(c, Component::ParentDir))
}

pub fn plan(snapshot: &LaunchSnapshot) -> Result<Value, CoreError> {
    plan_with(snapshot, None)
}

fn plan_with(snapshot: &LaunchSnapshot, sessions: Option<&Sessions>) -> Result<Value, CoreError> {
    if ![
        &snapshot.profile_id,
        &snapshot.launch_id,
        &snapshot.idempotency_key,
    ]
    .iter()
    .all(|s| valid_id(s))
    {
        return Err(CoreError::invalid(
            "Invalid profile, launch or idempotency identifier",
        ));
    }
    if let BuildPolicy::Pinned { sha256 } = &snapshot.build_policy
        && !core_engines::digest_string(sha256)
    {
        return Err(CoreError::invalid("Invalid pinned build digest"));
    }
    let r = &snapshot.resources;
    if ![&r.native_profile_dir, &r.data_root, &r.cache_root]
        .iter()
        .all(|s| absolute_lexical(s))
    {
        return Err(CoreError::invalid(
            "Explicit absolute profile, data and cache paths without traversal are required",
        ));
    }
    if matches!(snapshot.lifetime, Lifetime::Managed)
        && r.ownership_grant_ref.as_ref().is_none_or(|s| !valid_id(s))
    {
        return Err(CoreError::new(
            ErrorCode::AuthenticationFailure,
            "Managed planning requires an ownership grant reference",
        ));
    }
    if snapshot.theme.revision != 1 {
        return Err(CoreError::new(
            ErrorCode::ProtocolMismatch,
            "Unsupported theme revision",
        ));
    }
    if snapshot.startup_urls.len() > 32
        || snapshot.required_capabilities.len() > 64
        || snapshot.extensions.len() > 64
    {
        return Err(CoreError::new(
            ErrorCode::InsufficientResources,
            "Snapshot collection limit exceeded",
        ));
    }
    for text in &snapshot.startup_urls {
        if text.len() > 8192 || text.chars().any(char::is_control) {
            return Err(CoreError::invalid("Invalid startup URL"));
        }
        let u = url::Url::parse(text).map_err(|_| CoreError::invalid("Invalid startup URL"))?;
        if (!matches!(u.scheme(), "http" | "https") && text != "about:blank")
            || !u.username().is_empty()
            || u.password().is_some()
        {
            return Err(CoreError::invalid(
                "Startup URLs require HTTP(S) or about:blank without embedded credentials",
            ));
        }
    }
    let warnings = core_identity::validate(snapshot)?;
    let mut blockers = vec![
        json!({"code":"missing_package","message":"No approved owned engine package is installed"}),
    ];
    if !snapshot.required_capabilities.is_empty() {
        // There is no engine build against which required capabilities can be
        // negotiated. Unknown and known-but-untested capabilities both fail closed.
        blockers.push(json!({"code":"unsupported_capability","message":"Required engine capabilities are unavailable"}));
    }
    if let ProxyPolicy::Required {
        endpoint,
        secret_ref,
    } = &snapshot.proxy
    {
        if endpoint.len() > 2048 || endpoint.chars().any(char::is_control) {
            return Err(CoreError::invalid("Invalid proxy endpoint"));
        }
        let u =
            url::Url::parse(endpoint).map_err(|_| CoreError::invalid("Invalid proxy endpoint"))?;
        if !matches!(u.scheme(), "http" | "https" | "socks5" | "socks5h")
            || u.host_str().is_none()
            || u.port_or_known_default().is_none_or(|port| port == 0)
            || !u.username().is_empty()
            || u.password().is_some()
            || u.query().is_some()
            || u.fragment().is_some()
            || !matches!(u.path(), "" | "/")
            || secret_ref.as_ref().is_some_and(|s| !valid_id(s))
        {
            return Err(CoreError::invalid(
                "Proxy requires a host and nonzero effective port, with credentials delivered separately",
            ));
        }
        blockers.push(json!({"code":"unsupported_capability","message":"Required proxy transport is not implemented; direct fallback is forbidden"}));
    }
    if matches!(snapshot.lifetime, Lifetime::Detached) {
        blockers.push(json!({"code":"unsupported_capability","message":"Detached mode requires a surviving durable supervisor"}));
    }
    let mut selected = Value::Null;
    if let Some(sessions) = sessions {
        blockers.clear();
        match session::choose(&sessions.store, &sessions.root, snapshot) {
            Ok(package) => {
                selected = json!({"sha256":package.sha256,"upstream_revision":package.manifest.upstream_revision,
                    "target_triple":package.manifest.target_triple,"profile_data_format":package.manifest.profile_data_format});
            }
            Err(error) => blockers.push(json!(error)),
        }
    }
    Ok(
        json!({"status":if blockers.is_empty() {"planned"} else {"blocked"},"profile_id":snapshot.profile_id,"launch_id":snapshot.launch_id,
        "requested_engine":snapshot.engine,"selected_build":selected,"device_mode":snapshot.device_mode,
        "identity_key":core_identity::identity_key(snapshot),"identity_validation":"syntax-and-coherence-only",
        "settings_applied":false,"blockers":blockers,"warnings":warnings,
        "preparation_steps":[],"side_effects":false}),
    )
}

#[derive(Default)]
enum State {
    #[default]
    New,
    Hello,
    Ready,
}

#[derive(Default)]
pub struct Connection {
    state: State,
    ids: HashSet<String>,
    sessions: Option<Sessions>,
}
impl Connection {
    pub fn with_sessions(sessions: Sessions) -> Self {
        Self {
            sessions: Some(sessions),
            ..Self::default()
        }
    }

    fn disconnected() -> CoreError {
        CoreError::new(
            ErrorCode::UnsupportedCapability,
            "A configured native session authority is required",
        )
    }

    fn configured_capabilities(&self) -> Value {
        let mut report = capabilities();
        let Some(sessions) = &self.sessions else {
            return report;
        };
        report["features"]["updates.trusted"] = json!({"status":"partial","limitations":["Explicit offline publisher root and CLI package administration required"]});
        report["features"]["network.proxy.required"] = json!({"status":"partial","limitations":["Chromium supports a single explicit credential-free upstream; policy precedence and network acceptance need native verification"]});
        for operation in [
            "session.status",
            "session.list",
            "session.attach",
            "session.stop",
            "session.events",
        ] {
            report["features"][operation] = json!({"status":"supported","limitations":["Sessions created through this inherited connection"]});
        }
        if let Ok(package) = sessions
            .store
            .select(&EngineFamily::Chromium, &BuildPolicy::Approved)
        {
            report["scope"] = json!({"engine_build":package.sha256,"device_mode":"desktop","automation_transport":null});
            report["features"]["launch.plan"] = json!({"status":"supported","limitations":["Validates the selected signed browser and configuration without starting it"]});
            report["features"]["session.launch"] = json!({"status":"partial","limitations":["Standalone browser process, held for the lifetime of the connection"]});
            for capability in package.manifest.features.iter().filter(|feature| {
                matches!(feature.name.as_str(), "browser.navigation" | "browser.gui")
            }) {
                report["features"][&capability.name] = json!(capability);
            }
        }
        report
    }

    pub fn handle(&mut self, bytes: &[u8]) -> Response {
        let started = Instant::now();
        let request = match Request::decode(bytes) {
            Ok(r) => r,
            Err(e) => return Response::new(None, Err(e)),
        };
        let id = Some(request.id.clone());
        if self.ids.len() >= 1024 {
            return Response::new(
                id,
                Err(CoreError::new(
                    ErrorCode::InsufficientResources,
                    "Connection request limit reached; reconnect",
                )),
            );
        }
        if !self.ids.insert(request.id.clone()) {
            return Response::new(id, Err(CoreError::invalid("Duplicate request ID")));
        }
        let deadline = started + Duration::from_millis(request.deadline_ms as u64);
        let result = request
            .command()
            .and_then(|command| self.dispatch(command, deadline));
        if Instant::now() >= deadline && result.is_ok() {
            return Response::new(
                id,
                Err(CoreError::new(
                    ErrorCode::Cancellation,
                    "Request deadline exceeded",
                )),
            );
        }
        Response::new(id, result)
    }

    fn dispatch(&mut self, command: Command, deadline: Instant) -> Result<Value, CoreError> {
        if matches!(self.state, State::New) && !matches!(command, Command::Hello(_)) {
            return Err(CoreError::new(
                ErrorCode::ProtocolMismatch,
                "hello must be the first successful operation",
            ));
        }
        if let Command::Hello(hello) = command {
            if !matches!(self.state, State::New) {
                return Err(CoreError::invalid("Connection is already negotiated"));
            }
            if !hello.protocols.includes(PROTOCOL) || !hello.schemas.includes(SCHEMA_REVISION) {
                return Err(CoreError::new(
                    ErrorCode::ProtocolMismatch,
                    "No overlapping wire/schema version",
                ));
            }
            self.state = State::Hello;
            return Ok(
                json!({"product":"Numera Core Browser","core_version":env!("CARGO_PKG_VERSION"),"protocol":PROTOCOL,"schema_revision":SCHEMA_REVISION}),
            );
        }
        if let Command::CapabilitiesGet(_) = command {
            self.state = State::Ready;
            return Ok(self.configured_capabilities());
        }
        if !matches!(self.state, State::Ready) {
            return Err(CoreError::new(
                ErrorCode::ProtocolMismatch,
                "capabilities.get is required before other operations",
            ));
        }
        match command {
            Command::EnginesList(_) => self.sessions.as_ref().map_or_else(
                || Ok(core_engines::inventory()),
                |sessions| sessions.store.inventory(),
            ),
            Command::LaunchPlan(snapshot) => plan_with(&snapshot, self.sessions.as_ref()),
            Command::SessionLaunch(snapshot) => {
                if let Some(sessions) = &mut self.sessions {
                    return sessions.launch(snapshot, deadline);
                }
                plan(&snapshot)?;
                Err(CoreError::new(
                    ErrorCode::MissingPackage,
                    "No approved owned engine; session was not admitted or spawned",
                ))
            }
            Command::SessionStatus(reference) | Command::SessionAttach(reference) => self
                .sessions
                .as_mut()
                .ok_or_else(Self::disconnected)?
                .status(&reference.launch_id),
            Command::SessionList(_) => self
                .sessions
                .as_mut()
                .map_or_else(|| Ok(json!({"sessions":[]})), Sessions::list),
            Command::SessionStop(reference) => self
                .sessions
                .as_mut()
                .ok_or_else(Self::disconnected)?
                .stop(&reference.launch_id, deadline),
            Command::SessionEvents(query) => self
                .sessions
                .as_mut()
                .ok_or_else(Self::disconnected)?
                .events(&query.launch_id, query.after_sequence),
            Command::DiagnosticsCollect(_) => {
                let mut report = doctor();
                if let Some(sessions) = &self.sessions {
                    let engines = sessions.store.inventory()?;
                    report["owned_engine_available"] = json!(
                        sessions
                            .store
                            .select(&EngineFamily::Chromium, &BuildPolicy::Approved)
                            .is_ok()
                    );
                    report["engines"] = engines;
                    report["limitations"] = json!([
                        "Standalone sessions use an independently provisioned signed package",
                        "Manager grant and detached supervision require integration"
                    ]);
                }
                Ok(report)
            }
            Command::Hello(_) | Command::CapabilitiesGet(_) => unreachable!("handled above"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(id: u32, operation: &str, payload: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"namespace":NAMESPACE,"protocol":1,"id":id.to_string(),"deadline_ms":30000,"operation":operation,"payload":payload})).unwrap()
    }
    #[test]
    fn handshake_order_and_version_skew_fail_closed() {
        let mut c = Connection::default();
        assert!(serde_json::to_value(c.handle(&request(1,"engines.list",json!({})))).unwrap()["error"].is_object());
        let bad = json!({"protocols":{"min":2,"max":3},"schemas":{"min":1,"max":1}});
        assert!(
            serde_json::to_value(c.handle(&request(2, "hello", bad))).unwrap()["error"].is_object()
        );
        let hello = json!({"protocols":{"min":1,"max":1},"schemas":{"min":1,"max":1}});
        assert_eq!(
            serde_json::to_value(c.handle(&request(3, "hello", hello))).unwrap()["status"],
            "ok"
        );
        assert_eq!(
            serde_json::to_value(c.handle(&request(4, "engines.list", json!({})))).unwrap()["status"],
            "error"
        );
        c.handle(&request(5, "capabilities.get", json!({})));
        assert_eq!(
            serde_json::to_value(c.handle(&request(6, "engines.list", json!({})))).unwrap()["result"]
                ["installed"],
            json!([])
        );
    }
    #[test]
    fn duplicate_ids_and_unknown_fields_rejected() {
        let mut c = Connection::default();
        let hello = json!({"protocols":{"min":1,"max":1},"schemas":{"min":1,"max":1}});
        c.handle(&request(1, "hello", hello));
        assert_eq!(
            serde_json::to_value(c.handle(&request(1, "capabilities.get", json!({})))).unwrap()["error"]
                ["code"],
            "invalid_configuration"
        );
        assert_eq!(
            serde_json::to_value(c.handle(&request(
                2,
                "capabilities.get",
                json!({"secret":"synthetic"})
            )))
            .unwrap()["status"],
            "error"
        );
    }
}
