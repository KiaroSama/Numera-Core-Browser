use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};
use flate2::{Compression, write::GzEncoder};
use olpc_cjson::CanonicalFormatter;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

pub const EXPIRES: &str = "2099-01-01T00:00:00Z";

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn canonical(value: &Value) -> Vec<u8> {
    let mut bytes = Vec::new();
    value
        .serialize(&mut serde_json::Serializer::with_formatter(
            &mut bytes,
            CanonicalFormatter::new(),
        ))
        .unwrap();
    bytes
}

pub struct Key {
    pair: Ed25519KeyPair,
    id: String,
    metadata: Value,
}

impl Key {
    pub fn new(seed: u8) -> Self {
        let pair = Ed25519KeyPair::from_seed_unchecked(&[seed; 32]).unwrap();
        let public: String = pair
            .public_key()
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let metadata = json!({
            "keytype": "ed25519", "scheme": "ed25519", "keyval": {"public": public}
        });
        Self {
            pair,
            id: sha256(&canonical(&metadata)),
            metadata,
        }
    }

    pub fn root(&self, version: u32, signers: &[&Self]) -> Vec<u8> {
        let authority = json!({"keyids": [self.id], "threshold": 1});
        signed(
            json!({
                "_type": "root", "spec_version": "1.0.0", "version": version,
                "expires": EXPIRES, "consistent_snapshot": true,
                "keys": {&self.id: self.metadata},
                "roles": {
                    "root": authority, "targets": authority, "snapshot": authority,
                    "timestamp": authority
                }
            }),
            signers,
        )
    }
}

fn signed(metadata: Value, signers: &[&Key]) -> Vec<u8> {
    let bytes = canonical(&metadata);
    let signatures: Vec<_> = signers
        .iter()
        .map(|key| {
            let sig: String = key
                .pair
                .sign(&bytes)
                .as_ref()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            json!({"keyid": key.id, "sig": sig})
        })
        .collect();
    serde_json::to_vec(&json!({"signed": metadata, "signatures": signatures})).unwrap()
}

pub fn repository(
    path: &Path,
    key: &Key,
    version: u32,
    browser: &[u8],
    expires: &str,
) -> (String, String) {
    repository_with_profile_format(path, key, version, browser, expires, "fixture-only")
}

pub fn repository_with_profile_format(
    path: &Path,
    key: &Key,
    version: u32,
    browser: &[u8],
    expires: &str,
    profile_data_format: &str,
) -> (String, String) {
    let metadata = path.join("metadata");
    let targets = path.join("targets");
    fs::create_dir_all(&metadata).unwrap();
    fs::create_dir_all(&targets).unwrap();
    let (triple, executable) = if cfg!(windows) {
        ("x86_64-pc-windows-msvc", "chrome.exe")
    } else {
        ("x86_64-unknown-linux-gnu", "chrome")
    };
    let filename = format!("numera-chromium-{triple}.tar.gz");
    let members = [
        (executable, browser, 0o755),
        ("locales/داده.pak", b"assets", 0o644),
        ("licenses/chromium.LICENSE", b"test fixture license", 0o644),
    ];

    let mut tar = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    let mut inventory = std::collections::BTreeMap::new();
    for (name, bytes, mode) in members {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(mode);
        header.set_cksum();
        tar.append_data(&mut header, format!("runtime/{name}"), bytes)
            .unwrap();
        let digest = sha256(bytes);
        let line = format!(
            r#"{{"bytes":{},"sha256":"{digest}","mode":{}}}"#,
            bytes.len(),
            0o444 | (mode & 0o111)
        );
        inventory.insert(name, format!("{}:{line}", json!(name)));
    }
    let archive = tar.into_inner().unwrap().finish().unwrap();
    let inventory = format!(
        "{{{}}}",
        inventory.into_values().collect::<Vec<_>>().join(",")
    );
    let archive_hash = sha256(&archive);
    let archive_name = format!("{archive_hash}.{filename}");
    fs::write(targets.join(&archive_name), &archive).unwrap();

    let mut manifest: Value = serde_json::from_slice(include_bytes!(
        "../../../../contracts/fixtures/engine-manifest.json"
    ))
    .unwrap();
    manifest["target_triple"] = json!(triple);
    manifest["build_mode"] = json!("release");
    manifest["profile_data_format"] = json!(profile_data_format);
    manifest["executable"] = json!(executable);
    manifest["executable_sha256"] = json!(sha256(browser));
    let targets = signed(
        json!({
            "_type": "targets", "spec_version": "1.0.0", "version": version, "expires": expires,
            "targets": {
                filename: {
                    "length": archive.len(), "hashes": {"sha256": archive_hash},
                    "custom": {
                        "numera_engine_manifest": manifest,
                        "numera_runtime_inventory_sha256": sha256(inventory.as_bytes())
                    }
                }
            }
        }),
        &[key],
    );
    fs::write(metadata.join(format!("{version}.targets.json")), &targets).unwrap();
    let snapshot = signed(
        json!({
            "_type": "snapshot", "spec_version": "1.0.0", "version": version, "expires": expires,
            "meta": {
                "targets.json": {
                    "version": version, "length": targets.len(), "hashes": {"sha256": sha256(&targets)}
                }
            }
        }),
        &[key],
    );
    fs::write(metadata.join(format!("{version}.snapshot.json")), &snapshot).unwrap();
    let timestamp = signed(
        json!({
            "_type": "timestamp", "spec_version": "1.0.0", "version": version, "expires": expires,
            "meta": {
                "snapshot.json": {
                    "version": version, "length": snapshot.len(), "hashes": {"sha256": sha256(&snapshot)}
                }
            }
        }),
        &[key],
    );
    fs::write(metadata.join("timestamp.json"), timestamp).unwrap();
    (archive_hash, archive_name)
}
