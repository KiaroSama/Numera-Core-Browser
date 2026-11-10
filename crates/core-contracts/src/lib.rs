//! NCB-1: canonical types, strict JSON and bounded NDJSON framing.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{self, BufRead};

mod strict;
pub use strict::strict_json;

pub const NAMESPACE: &str = "numera.core";
pub const PROTOCOL: u16 = 1;
pub const SCHEMA_REVISION: u16 = 1;
/// Includes the terminating newline. Unframed EOF is an error.
pub const MAX_FRAME: usize = 1_048_576;
pub const MAX_DEADLINE_MS: u32 = 30_000;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub namespace: String,
    pub protocol: u16,
    pub id: String,
    pub deadline_ms: u32,
    pub operation: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VersionRange {
    pub min: u16,
    pub max: u16,
}
impl VersionRange {
    pub fn includes(&self, value: u16) -> bool {
        self.min <= value && value <= self.max
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub protocols: VersionRange,
    pub schemas: VersionRange,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EngineFamily {
    Chromium,
    Gecko,
    Webkit,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceMode {
    Desktop,
    AndroidEmulated,
    IosEmulated,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum BuildPolicy {
    Approved,
    Pinned { sha256: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub screen_width: u32,
    pub screen_height: u32,
    pub device_scale_factor: f64,
    pub touch: bool,
    pub language: String,
    pub timezone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ProxyPolicy {
    Direct,
    Required {
        endpoint: String,
        secret_ref: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Lifetime {
    Managed,
    Standalone,
    Detached,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceGrants {
    pub native_profile_dir: String,
    pub data_root: String,
    pub cache_root: String,
    pub ownership_grant_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeMode {
    Dark,
    Light,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    pub revision: u16,
    pub mode: ThemeMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchSnapshot {
    pub profile_id: String,
    pub launch_id: String,
    pub idempotency_key: String,
    pub engine: EngineFamily,
    pub build_policy: BuildPolicy,
    pub device_mode: DeviceMode,
    pub required_capabilities: Vec<String>,
    pub fingerprint_seed: String,
    pub identity: Identity,
    pub startup_urls: Vec<String>,
    pub resources: ResourceGrants,
    pub extensions: Vec<String>,
    pub lifetime: Lifetime,
    pub proxy: ProxyPolicy,
    pub theme: Theme,
    /// Explicit testing-only acceptance; never claims a real mobile OS.
    pub accept_emulation_limitations: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "operation", content = "payload")]
pub enum Command {
    #[serde(rename = "hello")]
    Hello(Hello),
    #[serde(rename = "capabilities.get")]
    CapabilitiesGet(Empty),
    #[serde(rename = "engines.list")]
    EnginesList(Empty),
    #[serde(rename = "launch.plan")]
    LaunchPlan(LaunchSnapshot),
    #[serde(rename = "session.launch")]
    SessionLaunch(LaunchSnapshot),
    #[serde(rename = "diagnostics.collect")]
    DiagnosticsCollect(Empty),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    ProtocolMismatch,
    UnsupportedCapability,
    IncompatibleEngineDataFormat,
    MissingPackage,
    OwnershipConflict,
    AdmissionTimeout,
    AuthenticationFailure,
    ProxyFailure,
    InvalidConfiguration,
    UpdateSignatureFailure,
    InsufficientResources,
    LaunchFailure,
    Cancellation,
    RecoveryRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CoreError {
    pub code: ErrorCode,
    pub message: String,
}
impl CoreError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub fn invalid(message: &'static str) -> Self {
        Self::new(ErrorCode::InvalidConfiguration, message)
    }
}
impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}
impl std::error::Error for CoreError {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Ok { result: Value },
    Error { error: CoreError },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Response {
    pub namespace: String,
    pub protocol: u16,
    pub id: Option<String>,
    #[serde(flatten)]
    pub outcome: Outcome,
}
impl Response {
    pub fn new(id: Option<String>, outcome: Result<Value, CoreError>) -> Self {
        Self {
            namespace: NAMESPACE.into(),
            protocol: PROTOCOL,
            id,
            outcome: match outcome {
                Ok(result) => Outcome::Ok { result },
                Err(error) => Outcome::Error { error },
            },
        }
    }
}

impl Request {
    pub fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        let value = strict_json(bytes)?;
        let request: Self = serde_json::from_value(value)
            .map_err(|_| CoreError::invalid("Invalid request envelope"))?;
        if request.namespace != NAMESPACE || request.protocol != PROTOCOL {
            return Err(CoreError::new(
                ErrorCode::ProtocolMismatch,
                "Expected numera.core wire major 1",
            ));
        }
        if !valid_id(&request.id) || !(1..=MAX_DEADLINE_MS).contains(&request.deadline_ms) {
            return Err(CoreError::invalid(
                "Invalid request ID or relative deadline",
            ));
        }
        Ok(request)
    }
    pub fn command(&self) -> Result<Command, CoreError> {
        serde_json::from_value(json!({"operation": self.operation, "payload": self.payload}))
            .map_err(|_| CoreError::invalid("Unknown operation or invalid typed payload"))
    }
}

pub fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

pub fn read_frame(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Incomplete NDJSON frame",
                ))
            };
        }
        let count = available
            .iter()
            .position(|b| *b == b'\n')
            .map_or(available.len(), |n| n + 1);
        if bytes.len() + count > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "NDJSON frame exceeds 1 MiB",
            ));
        }
        bytes.extend_from_slice(&available[..count]);
        let complete = available[count - 1] == b'\n';
        reader.consume(count);
        if complete {
            bytes.pop();
            return Ok(Some(bytes));
        }
    }
}

/// The command branches and payload definitions are generated from the same
/// Rust types the producer consumes. Envelope limits are normative as well.
pub fn request_schema() -> Value {
    let mut envelope =
        serde_json::to_value(schemars::schema_for!(Request)).expect("Schema is serializable");
    let commands =
        serde_json::to_value(schemars::schema_for!(Command)).expect("Schema is serializable");
    envelope["$id"] = json!("https://numera.invalid/contracts/v1/request.schema.json");
    envelope["properties"]["namespace"] = json!({"const": NAMESPACE});
    envelope["properties"]["protocol"] = json!({"const": PROTOCOL});
    envelope["properties"]["id"] =
        json!({"type":"string", "minLength":1,"maxLength":128,"pattern":"^[A-Za-z0-9._-]+$"});
    envelope["properties"]["deadline_ms"] =
        json!({"type":"integer","minimum":1,"maximum":MAX_DEADLINE_MS});
    envelope["$defs"] = commands["$defs"].clone();
    envelope["oneOf"] = commands["oneOf"].clone();
    envelope
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    #[test]
    fn adjacent_frames_and_clean_eof() {
        let mut r = BufReader::with_capacity(2, Cursor::new(b"{}\n[]\n"));
        assert_eq!(read_frame(&mut r).unwrap(), Some(b"{}".to_vec()));
        assert_eq!(read_frame(&mut r).unwrap(), Some(b"[]".to_vec()));
        assert!(read_frame(&mut r).unwrap().is_none());
    }
    #[test]
    fn truncated_frame_is_never_accepted() {
        assert_eq!(
            read_frame(&mut Cursor::new(b"{}")).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
    #[test]
    fn frame_limit_includes_newline() {
        let mut exact = vec![b' '; MAX_FRAME - 1];
        exact.push(b'\n');
        assert!(read_frame(&mut Cursor::new(exact)).is_ok());
        let mut excess = vec![b' '; MAX_FRAME];
        excess.push(b'\n');
        assert!(read_frame(&mut Cursor::new(excess)).is_err());
    }
    #[test]
    fn schema_has_typed_command_branches() {
        let s = request_schema();
        assert!(s["oneOf"].as_array().is_some_and(|v| v.len() == 6));
        assert!(s["$defs"]["LaunchSnapshot"].is_object());
    }
    #[test]
    fn ids_are_safe_for_correlation() {
        for bad in ["", "a\nERROR token", "https://u:password@host", "شناسه"] {
            assert!(!valid_id(bad));
        }
        assert!(valid_id("session-01_test.id"));
    }
}
