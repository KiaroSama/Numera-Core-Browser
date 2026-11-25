use crate::{ClientError, TransportError, pipe::remaining};
use core_contracts::{
    Command, CoreError, DeviceMode, EngineFamily, ErrorCode, MAX_DEADLINE_MS, MAX_FRAME, NAMESPACE,
    Outcome, PROTOCOL, Response, SCHEMA_REVISION, strict_json, valid_id,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{collections::BTreeMap, env, io, time::Instant};

#[derive(Serialize)]
struct Envelope<'a> {
    namespace: &'static str,
    protocol: u16,
    id: &'a str,
    deadline_ms: u32,
    #[serde(flatten)]
    command: &'a Command,
}

#[derive(Default)]
struct Frame(Vec<u8>);

impl io::Write for Frame {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() >= MAX_FRAME - self.0.len() {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn encode(
    command: &Command,
    id: &str,
    deadline: Instant,
) -> Result<Vec<u8>, ClientError> {
    let deadline_ms = remaining(deadline)?
        .as_nanos()
        .div_ceil(1_000_000)
        .min(u128::from(MAX_DEADLINE_MS)) as u32;
    let request = Envelope {
        namespace: NAMESPACE,
        protocol: PROTOCOL,
        id,
        deadline_ms,
        command,
    };
    let mut frame = Frame::default();
    serde_json::to_writer(&mut frame, &request).map_err(|error| {
        ClientError::Request(
            if error.io_error_kind() == Some(io::ErrorKind::FileTooLarge) {
                ErrorCode::InsufficientResources
            } else {
                ErrorCode::InvalidConfiguration
            },
        )
    })?;
    frame.0.push(b'\n');
    Ok(frame.0)
}

pub(crate) fn decode<T: DeserializeOwned>(frame: &[u8], id: &str) -> Result<T, ClientError> {
    let value = strict_json(frame).map_err(|_| TransportError::InvalidResponse)?;
    let envelope = value.as_object().ok_or(TransportError::InvalidResponse)?;
    if envelope.get("namespace").and_then(Value::as_str) != Some(NAMESPACE)
        || envelope.get("protocol").and_then(Value::as_u64) != Some(u64::from(PROTOCOL))
        || envelope.get("id").and_then(Value::as_str) != Some(id)
    {
        return Err(TransportError::MismatchedResponse.into());
    }
    let payload = match envelope.get("status").and_then(Value::as_str) {
        Some("ok") => "result",
        Some("error") => "error",
        _ => return Err(TransportError::InvalidResponse.into()),
    };
    if envelope.len() != 5 || !envelope.get(payload).is_some_and(Value::is_object) {
        return Err(TransportError::InvalidResponse.into());
    }
    let response: Response =
        serde_json::from_value(value).map_err(|_| TransportError::InvalidResponse)?;
    match response.outcome {
        Outcome::Error { error } => Err(ClientError::Request(error.code)),
        Outcome::Ok { result } => {
            serde_json::from_value(result).map_err(|_| TransportError::InvalidResponse.into())
        }
    }
}

pub(crate) fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Deserialize)]
pub(crate) struct HelloReply {
    product: String,
    pub(crate) core_version: String,
    protocol: u16,
    schema_revision: u16,
}

impl HelloReply {
    pub(crate) fn validate(&self) -> Result<(), TransportError> {
        if self.product != "Numera Core Browser"
            || self.protocol != PROTOCOL
            || self.schema_revision != SCHEMA_REVISION
        {
            return Err(TransportError::InvalidResponse);
        }
        Ok(())
    }
}

#[derive(Deserialize)]
pub struct Capabilities {
    pub namespace: String,
    pub core_version: String,
    pub protocol: u16,
    pub schema_revision: u16,
    pub host: Host,
    pub scope: CapabilityScope,
    pub features: BTreeMap<String, Capability>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Host {
    pub os: String,
    pub arch: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityScope {
    #[serde(deserialize_with = "Option::deserialize")]
    pub engine_build: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub device_mode: Option<DeviceMode>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub automation_transport: Option<String>,
}

#[derive(Deserialize)]
pub struct Capability {
    pub status: CapabilityStatus,
    #[serde(flatten)]
    pub details: BTreeMap<String, Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Supported,
    Partial,
    Unsupported,
}

impl Capabilities {
    pub(crate) fn validate(&self, hello: &HelloReply) -> Result<(), TransportError> {
        if self.namespace != NAMESPACE
            || self.protocol != PROTOCOL
            || self.schema_revision != SCHEMA_REVISION
            || self.core_version != hello.core_version
            || self.host.os != env::consts::OS
            || self.host.arch != env::consts::ARCH
            || self
                .scope
                .engine_build
                .as_deref()
                .is_some_and(|digest| !valid_digest(digest))
            || self.support("protocol.ndjson") != Some(CapabilityStatus::Supported)
        {
            return Err(TransportError::InvalidCapabilityScope);
        }
        Ok(())
    }

    pub fn support(&self, name: &str) -> Option<CapabilityStatus> {
        self.features.get(name).map(|feature| feature.status)
    }
}

#[derive(Deserialize)]
pub struct LaunchPlan {
    pub status: PlanStatus,
    pub launch_id: String,
    pub profile_id: String,
    pub requested_engine: EngineFamily,
    pub device_mode: DeviceMode,
    pub selected_build: Option<SelectedBuild>,
    pub blockers: Vec<CoreError>,
    pub identity_key: String,
    pub identity_validation: String,
    pub settings_applied: bool,
    pub side_effects: bool,
    pub warnings: Vec<String>,
    pub preparation_steps: Vec<Value>,
}

#[derive(Deserialize)]
pub struct SelectedBuild {
    pub sha256: String,
    pub upstream_revision: String,
    pub target_triple: String,
    pub profile_data_format: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Planned,
    Blocked,
}

#[derive(Clone, Deserialize, PartialEq, Eq)]
pub struct SessionView {
    pub launch_id: String,
    pub profile_id: String,
    pub package_sha256: String,
    pub device_mode: DeviceMode,
    pub state: SessionState,
    pub sequence: u64,
    pub browser_process_started: bool,
    pub automation_endpoints: Vec<String>,
}

impl SessionView {
    pub(crate) fn validate(&self, expected_id: Option<&str>) -> Result<(), TransportError> {
        if !valid_id(&self.launch_id)
            || !valid_id(&self.profile_id)
            || !valid_digest(&self.package_sha256)
            || expected_id.is_some_and(|id| id != self.launch_id)
            || self.device_mode != DeviceMode::Desktop
            || self.sequence == 0
        {
            return Err(TransportError::InvalidResponse);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Running,
    Stopped,
    RecoveryRequired,
}

#[derive(Deserialize)]
pub struct SessionEvents {
    pub events: Vec<SessionView>,
    pub next_sequence: u64,
}

#[derive(Deserialize)]
pub(crate) struct SessionList {
    pub(crate) sessions: Vec<SessionView>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scope_requires_an_explicit_value_for_each_nullable_field() {
        let mut scope = json!({"engine_build":null,"device_mode":null,"automation_transport":null});
        assert!(serde_json::from_value::<CapabilityScope>(scope.clone()).is_ok());
        scope
            .as_object_mut()
            .unwrap()
            .remove("automation_transport");
        assert!(serde_json::from_value::<CapabilityScope>(scope).is_err());
    }

    #[test]
    fn core_error_reports_the_code_without_printing_peer_data() {
        let reply = br#"{"namespace":"numera.core","protocol":1,"id":"manager-1","status":"error","error":{"code":"missing_package","message":"private-placeholder"}}"#;
        let error = decode::<Value>(reply, "manager-1").unwrap_err();
        assert_eq!(error, ClientError::Request(ErrorCode::MissingPackage));
        assert!(!format!("{error} {error:?}").contains("private-placeholder"));
    }
}
