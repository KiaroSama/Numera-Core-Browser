//! Untrusted package metadata inspection. Hash matching is not trust approval.
use core_contracts::{CoreError, EngineFamily, ErrorCode, strict_json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::{Component, Path},
};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EngineManifest {
    pub schema_revision: u16,
    pub package_id: String,
    pub family: EngineFamily,
    pub upstream_revision: String,
    pub upstream_source_url: String,
    pub numera_patch_revision: String,
    pub patch_series_sha256: String,
    pub target_triple: String,
    pub build_mode: String,
    pub executable: String,
    pub executable_sha256: String,
    pub source_archive_sha256: String,
    pub source_archive_url: String,
    pub core_version_range: String,
    pub profile_data_format: String,
    pub licenses: Vec<String>,
    pub features: Vec<FeatureCapability>,
    pub build_evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Support {
    Supported,
    Partial,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FeatureCapability {
    pub name: String,
    pub status: Support,
    pub limitations: Vec<String>,
    pub evidence: Vec<String>,
}

fn public_source_url(text: &str) -> bool {
    url::Url::parse(text).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none()
    })
}

pub fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.contains(':')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && Path::new(path)
            .components()
            .all(|p| matches!(p, Component::Normal(_)))
}
pub fn digest_string(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn inspect_manifest(bytes: &[u8]) -> Result<EngineManifest, CoreError> {
    let m: EngineManifest = serde_json::from_value(strict_json(bytes)?)
        .map_err(|_| CoreError::invalid("Invalid engine manifest"))?;
    let package = match m.family {
        EngineFamily::Chromium => "numera-chromium",
        EngineFamily::Gecko => "numera-gecko",
        EngineFamily::Webkit => "numera-webkit",
    };
    if m.schema_revision != 1
        || m.package_id != package
        || !safe_relative(&m.executable)
        || !digest_string(&m.executable_sha256)
        || !digest_string(&m.source_archive_sha256)
        || !digest_string(&m.patch_series_sha256)
        || !public_source_url(&m.upstream_source_url)
        || !public_source_url(&m.source_archive_url)
        || m.upstream_revision.is_empty()
        || m.numera_patch_revision.is_empty()
        || m.target_triple.is_empty()
        || !["release", "development"].contains(&m.build_mode.as_str())
        || m.licenses.is_empty()
        || !m.licenses.iter().all(|path| safe_relative(path))
        || m.core_version_range.is_empty()
        || m.profile_data_format.is_empty()
        || m.features.iter().any(|f| {
            f.name.is_empty()
                || (matches!(f.status, Support::Supported | Support::Partial)
                    && f.evidence.is_empty())
        })
        || m.build_evidence.is_empty()
    {
        return Err(CoreError::invalid("Inconsistent or unsafe engine manifest"));
    }
    Ok(m)
}

pub fn hash_file(path: &Path) -> Result<String, CoreError> {
    if !std::fs::metadata(path).is_ok_and(|m| m.is_file()) {
        return Err(CoreError::invalid("Hash input must be a regular file"));
    }
    let mut file = File::open(path)
        .map_err(|_| CoreError::new(ErrorCode::MissingPackage, "Cannot read the selected file"))?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| CoreError::invalid("Cannot hash the selected file"))?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

pub fn inventory() -> serde_json::Value {
    serde_json::json!({"installed": [], "available_approved": [], "families": [
        {"family":"chromium","package_id":"numera-chromium","status":"blocked","reason":"No owned source-built and approved engine package exists"},
        {"family":"gecko","package_id":"numera-gecko","status":"deferred","reason":"Future engine path after Chromium milestone"},
        {"family":"webkit","package_id":"numera-webkit","status":"deferred","reason":"Future engine path after Chromium milestone"}
    ]})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cross_platform_path_escapes_rejected() {
        for path in [
            "",
            "../x",
            "a/../../x",
            "/bin/x",
            "C:/x",
            "C:\\x",
            "a\\..\\x",
            "//host/x",
            "a//x",
            "a/./x",
            "a\0x",
        ] {
            assert!(!safe_relative(path), "{path:?}");
        }
        assert!(safe_relative("bin/numera-chromium"));
    }
    #[test]
    fn empty_inventory_does_not_claim_a_browser() {
        assert_eq!(inventory()["installed"], serde_json::json!([]));
    }

    #[test]
    fn manifest_consistency_and_evidence_fail_closed() {
        let bytes = include_bytes!("../../../contracts/fixtures/engine-manifest.json");
        assert!(inspect_manifest(bytes).is_ok());
        for index in 0..5 {
            let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
            match index {
                0 => value["package_id"] = serde_json::json!("numera-gecko"),
                1 => value["licenses"] = serde_json::json!(["../LICENSE"]),
                2 => value["features"][0]["status"] = serde_json::json!("supported"),
                3 => {
                    value["source_archive_url"] = serde_json::json!("http://example.invalid/source")
                }
                _ => value["executable"] = serde_json::json!("../../outside"),
            }
            assert!(inspect_manifest(&serde_json::to_vec(&value).unwrap()).is_err());
        }
    }
}
