//! Client for the inherited-stdio NCB-1 development protocol.

mod error;
mod pipe;
mod protocol;

pub use error::{ClientError, TransportError};
pub use protocol::{
    Capabilities, Capability, CapabilityScope, CapabilityStatus, Host, LaunchPlan, PlanStatus,
    SelectedBuild, SessionEvents, SessionState, SessionView,
};

use core_contracts::{
    BuildPolicy, Command, DeviceMode, Empty, EngineFamily, ErrorCode, Hello, LaunchSnapshot,
    Lifetime, MAX_DEADLINE_MS, PROTOCOL, SCHEMA_REVISION, SessionEventQuery, SessionReference,
    VersionRange, valid_id,
};
use pipe::{Pipes, remaining};
use protocol::{HelloReply, SessionList, valid_digest};
use serde::de::DeserializeOwned;
use std::{
    collections::HashSet,
    path::Path,
    process::ExitStatus,
    time::{Duration, Instant},
};

const MAX_REQUESTS: u16 = 1024;

fn deadline(milliseconds: u32) -> Result<Instant, ClientError> {
    if !(1..=MAX_DEADLINE_MS).contains(&milliseconds) {
        return Err(ClientError::Request(ErrorCode::InvalidConfiguration));
    }
    Ok(Instant::now() + Duration::from_millis(u64::from(milliseconds)))
}

fn reference(id: &str) -> SessionReference {
    SessionReference {
        launch_id: id.to_owned(),
    }
}

struct Connection {
    pipes: Pipes,
    sent: u16,
}

impl Connection {
    fn request<T: DeserializeOwned>(
        &mut self,
        command: Command,
        deadline: Instant,
    ) -> Result<T, ClientError> {
        if self.sent == MAX_REQUESTS {
            return Err(ClientError::Request(ErrorCode::InsufficientResources));
        }
        let id = format!("manager-{}", self.sent + 1);
        let request = protocol::encode(&command, &id, deadline)?;
        self.sent += 1;
        let frame = self.pipes.exchange(&request, deadline)?;
        let result = protocol::decode(&frame, &id);
        remaining(deadline)?;
        result
    }
}

/// Owns the Core service and one sequential inherited-stdio connection.
///
/// Request errors keep the connection usable. Transport errors invalidate it
/// and may leave the outcome unknown. Dropping the client delivers EOF to Core.
pub struct CoreClient {
    connection: Connection,
    capabilities: Capabilities,
}

impl CoreClient {
    /// Start and negotiate with the selected Core executable and configured store.
    pub fn connect(
        binary: &Path,
        store: &Path,
        data_root: &Path,
        deadline_ms: u32,
    ) -> Result<Self, ClientError> {
        let deadline = deadline(deadline_ms)?;
        if [binary, store, data_root]
            .iter()
            .any(|path| !path.is_absolute())
        {
            return Err(ClientError::Request(ErrorCode::InvalidConfiguration));
        }
        let mut connection = Connection {
            pipes: Pipes::open(binary, store, data_root)?,
            sent: 0,
        };
        let hello: HelloReply = connection.request(
            Command::Hello(Hello {
                protocols: VersionRange {
                    min: PROTOCOL,
                    max: PROTOCOL,
                },
                schemas: VersionRange {
                    min: SCHEMA_REVISION,
                    max: SCHEMA_REVISION,
                },
            }),
            deadline,
        )?;
        hello.validate()?;
        let capabilities: Capabilities =
            connection.request(Command::CapabilitiesGet(Empty {}), deadline)?;
        capabilities.validate(&hello)?;
        Ok(Self {
            connection,
            capabilities,
        })
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// Send EOF and wait up to eight seconds for Core. Reaping continues on timeout.
    pub fn close(&mut self) -> Result<ExitStatus, ClientError> {
        self.connection.pipes.close().map_err(Into::into)
    }

    fn operation<T>(
        &mut self,
        milliseconds: u32,
        run: impl FnOnce(&mut Self, Instant) -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        let deadline = deadline(milliseconds)?;
        if !self.connection.pipes.is_open() {
            return Err(TransportError::Closed.into());
        }
        let result = run(self, deadline);
        if matches!(result, Err(ClientError::Transport(_))) {
            self.connection.pipes.shutdown();
        }
        result
    }

    fn plan_for(
        &mut self,
        snapshot: &LaunchSnapshot,
        deadline: Instant,
    ) -> Result<LaunchPlan, ClientError> {
        if !snapshot.identity.device_scale_factor.is_finite() {
            return Err(ClientError::Request(ErrorCode::InvalidConfiguration));
        }
        let plan: LaunchPlan = self
            .connection
            .request(Command::LaunchPlan(snapshot.clone()), deadline)?;
        if plan.launch_id != snapshot.launch_id
            || plan.profile_id != snapshot.profile_id
            || plan.requested_engine != snapshot.engine
            || plan.device_mode != snapshot.device_mode
            || (plan.status == PlanStatus::Blocked) == plan.blockers.is_empty()
        {
            return Err(TransportError::InvalidResponse.into());
        }
        Ok(plan)
    }

    pub fn plan(
        &mut self,
        snapshot: &LaunchSnapshot,
        deadline_ms: u32,
    ) -> Result<LaunchPlan, ClientError> {
        self.operation(deadline_ms, |client, deadline| {
            client.plan_for(snapshot, deadline)
        })
    }

    /// Plan and launch one standalone Chromium desktop snapshot with a pinned digest.
    pub fn launch(
        &mut self,
        snapshot: &LaunchSnapshot,
        deadline_ms: u32,
    ) -> Result<SessionView, ClientError> {
        self.operation(deadline_ms, |client, deadline| {
            let BuildPolicy::Pinned { sha256: digest } = &snapshot.build_policy else {
                return Err(ClientError::Request(ErrorCode::InvalidConfiguration));
            };
            if !valid_digest(digest) {
                return Err(ClientError::Request(ErrorCode::InvalidConfiguration));
            }
            if !matches!(snapshot.lifetime, Lifetime::Standalone)
                || snapshot.device_mode != DeviceMode::Desktop
                || snapshot.engine != EngineFamily::Chromium
            {
                return Err(ClientError::Request(ErrorCode::UnsupportedCapability));
            }

            let plan = client.plan_for(snapshot, deadline)?;
            if let Some(blocker) = plan.blockers.into_iter().next() {
                return Err(ClientError::Request(blocker.code));
            }
            if plan
                .selected_build
                .as_ref()
                .is_none_or(|selected| selected.sha256 != *digest)
            {
                return Err(TransportError::InvalidResponse.into());
            }
            let capabilities = &client.capabilities;
            if capabilities.scope.engine_build.as_ref() != Some(digest)
                || capabilities.scope.device_mode.as_ref() != Some(&snapshot.device_mode)
                || capabilities.scope.automation_transport.is_some()
                || !matches!(
                    capabilities.support("session.launch"),
                    Some(CapabilityStatus::Supported | CapabilityStatus::Partial)
                )
                || snapshot
                    .required_capabilities
                    .iter()
                    .any(|name| capabilities.support(name) != Some(CapabilityStatus::Supported))
            {
                return Err(ClientError::Request(ErrorCode::UnsupportedCapability));
            }

            let result: SessionView = client
                .connection
                .request(Command::SessionLaunch(snapshot.clone()), deadline)?;
            result.validate(Some(&snapshot.launch_id))?;
            if result.profile_id != snapshot.profile_id
                || result.package_sha256 != *digest
                || result.device_mode != snapshot.device_mode
                || !result.browser_process_started
            {
                return Err(TransportError::InvalidResponse.into());
            }
            Ok(result)
        })
    }

    fn session_request<T: DeserializeOwned>(
        &mut self,
        command: Command,
        deadline: Instant,
    ) -> Result<T, ClientError> {
        let (operation, launch_id) = match &command {
            Command::SessionAttach(reference) => ("session.attach", Some(&reference.launch_id)),
            Command::SessionStatus(reference) => ("session.status", Some(&reference.launch_id)),
            Command::SessionList(_) => ("session.list", None),
            Command::SessionStop(reference) => ("session.stop", Some(&reference.launch_id)),
            Command::SessionEvents(query) => ("session.events", Some(&query.launch_id)),
            _ => return Err(ClientError::Request(ErrorCode::InvalidConfiguration)),
        };
        if self.capabilities.support(operation) != Some(CapabilityStatus::Supported) {
            return Err(ClientError::Request(ErrorCode::UnsupportedCapability));
        }
        if launch_id.is_some_and(|id| !valid_id(id)) {
            return Err(ClientError::Request(ErrorCode::InvalidConfiguration));
        }
        self.connection.request(command, deadline)
    }

    pub fn attach(
        &mut self,
        launch_id: &str,
        deadline_ms: u32,
    ) -> Result<SessionView, ClientError> {
        self.session(
            Command::SessionAttach(reference(launch_id)),
            launch_id,
            deadline_ms,
        )
    }

    pub fn status(
        &mut self,
        launch_id: &str,
        deadline_ms: u32,
    ) -> Result<SessionView, ClientError> {
        self.session(
            Command::SessionStatus(reference(launch_id)),
            launch_id,
            deadline_ms,
        )
    }

    pub fn stop(&mut self, launch_id: &str, deadline_ms: u32) -> Result<SessionView, ClientError> {
        self.session(
            Command::SessionStop(reference(launch_id)),
            launch_id,
            deadline_ms,
        )
    }

    fn session(
        &mut self,
        command: Command,
        launch_id: &str,
        deadline_ms: u32,
    ) -> Result<SessionView, ClientError> {
        let stopping = matches!(command, Command::SessionStop(_));
        self.operation(deadline_ms, |client, deadline| {
            let result: SessionView = client.session_request(command, deadline)?;
            result.validate(Some(launch_id))?;
            if stopping && result.state != SessionState::Stopped {
                return Err(TransportError::InvalidResponse.into());
            }
            Ok(result)
        })
    }

    pub fn list(&mut self, deadline_ms: u32) -> Result<Vec<SessionView>, ClientError> {
        self.operation(deadline_ms, |client, deadline| {
            let result: SessionList =
                client.session_request(Command::SessionList(Empty {}), deadline)?;
            let mut handles = HashSet::new();
            for session in &result.sessions {
                session.validate(None)?;
                if !handles.insert(&session.launch_id) {
                    return Err(TransportError::InvalidResponse.into());
                }
            }
            Ok(result.sessions)
        })
    }

    pub fn events(
        &mut self,
        launch_id: &str,
        after_sequence: u64,
        deadline_ms: u32,
    ) -> Result<SessionEvents, ClientError> {
        self.operation(deadline_ms, |client, deadline| {
            let query = SessionEventQuery {
                launch_id: launch_id.to_owned(),
                after_sequence,
            };
            let result: SessionEvents =
                client.session_request(Command::SessionEvents(query), deadline)?;
            if result.next_sequence.checked_sub(after_sequence) != Some(result.events.len() as u64)
            {
                return Err(TransportError::EventGap.into());
            }
            for (offset, event) in result.events.iter().enumerate() {
                event.validate(Some(launch_id))?;
                if event.sequence != after_sequence + offset as u64 + 1 {
                    return Err(TransportError::EventGap.into());
                }
            }
            Ok(result)
        })
    }
}
