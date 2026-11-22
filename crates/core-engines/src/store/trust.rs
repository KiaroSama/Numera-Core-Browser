use super::{
    PackageInfo,
    storage::{
        absolute_directory, bounded_read, bytes, directory, missing, parse, recovery, sha256,
        signature, storage_error,
    },
};
use crate::{digest_string, inspect_manifest, runtime_archive::MAX_ARCHIVE_BYTES};
use core_contracts::{CoreError, EngineFamily, ErrorCode, MAX_FRAME, strict_json};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};
use tough::{
    Limits, RepositoryLoader, TargetName, Transport, TransportError, TransportErrorKind,
    TransportStream,
    schema::{Role, Root, Signed, Snapshot, Targets, Timestamp},
};
use url::Url;

const MAX_METADATA_BYTES: u64 = (MAX_FRAME - 1) as u64;
const MAX_ROOT_UPDATES: u64 = 128;
const CACHE: [&str; 5] = [
    "root.json",
    "timestamp.json",
    "snapshot.json",
    "targets.json",
    "latest_known_time.json",
];

pub(super) fn platform() -> Result<&'static str, CoreError> {
    if cfg!(all(
        target_os = "linux",
        target_arch = "x86_64",
        target_env = "gnu"
    )) {
        Ok("x86_64-unknown-linux-gnu")
    } else if cfg!(all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc"
    )) {
        Ok("x86_64-pc-windows-msvc")
    } else {
        Err(CoreError::new(
            ErrorCode::UnsupportedCapability,
            "The package store requires a supported x86-64 Chromium host",
        ))
    }
}

fn target_name() -> Result<String, CoreError> {
    Ok(format!("numera-chromium-{}.tar.gz", platform()?))
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Proof {
    pub(super) root: Signed<Root>,
    pub(super) targets: Signed<Targets>,
}

pub(super) struct Authorized {
    pub(super) package: PackageInfo,
    pub(super) inventory: String,
}

impl Proof {
    pub(super) fn authorized(&self) -> Result<Authorized, CoreError> {
        self.root
            .signed
            .verify_role(&self.root)
            .map_err(|_| signature())?;
        self.root
            .signed
            .verify_role(&self.targets)
            .map_err(|_| signature())?;
        if self.targets.signed.delegations.is_some() {
            return Err(signature());
        }
        let name = TargetName::new(target_name()?).map_err(|_| signature())?;
        let target = self.targets.signed.targets.get(&name).ok_or_else(missing)?;
        let hash = &target.hashes.sha256;
        if hash.len() != 32 || target.length == 0 || target.length > MAX_ARCHIVE_BYTES {
            return Err(signature());
        }
        let manifest_value = target
            .custom
            .get("numera_engine_manifest")
            .ok_or_else(signature)?;
        let manifest = inspect_manifest(&bytes(manifest_value)?).map_err(|_| signature())?;
        validate_manifest(&manifest)?;
        if manifest.target_triple != platform()? {
            return Err(CoreError::new(
                ErrorCode::IncompatibleEngineDataFormat,
                "The signed Chromium package is incompatible with this Core build or host",
            ));
        }
        let inventory = target
            .custom
            .get("numera_runtime_inventory_sha256")
            .and_then(serde_json::Value::as_str)
            .filter(|digest| digest_string(digest))
            .ok_or_else(signature)?
            .to_ascii_lowercase();
        let sha256 = hash.iter().map(|byte| format!("{byte:02x}")).collect();
        Ok(Authorized {
            package: PackageInfo {
                sha256,
                archive_bytes: target.length,
                manifest,
            },
            inventory,
        })
    }

    pub(super) fn archive_name(&self, digest: &str) -> Result<String, CoreError> {
        let name = target_name()?;
        if self.root.signed.consistent_snapshot {
            Ok(format!("{digest}.{name}"))
        } else {
            Ok(name)
        }
    }
}

pub(super) fn validate_manifest(manifest: &crate::EngineManifest) -> Result<(), CoreError> {
    let core = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|_| recovery())?;
    let range = VersionReq::parse(&manifest.core_version_range).map_err(|_| signature())?;
    if manifest.family != EngineFamily::Chromium
        || !matches!(
            manifest.target_triple.as_str(),
            "x86_64-unknown-linux-gnu" | "x86_64-pc-windows-msvc"
        )
        || manifest.build_mode != "release"
        || !range.matches(&core)
        || (manifest.target_triple == "x86_64-pc-windows-msvc"
            && !manifest.executable.ends_with(".exe"))
    {
        return Err(CoreError::new(
            ErrorCode::IncompatibleEngineDataFormat,
            "The signed Chromium package is incompatible with this Core build or host",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Trust {
    anchor: String,
    roles: BTreeMap<String, String>,
    complete: bool,
}

impl Trust {
    fn read<T: serde::de::DeserializeOwned>(&self, name: &str) -> Result<T, CoreError> {
        let data = self.roles.get(name).ok_or_else(recovery)?;
        parse(data.as_bytes())
    }

    pub(super) fn current(&self) -> Result<Proof, CoreError> {
        if !self.complete {
            return Err(signature());
        }
        let root: Signed<Root> = self.read("root.json")?;
        let timestamp: Signed<Timestamp> = self.read("timestamp.json")?;
        let snapshot: Signed<Snapshot> = self.read("snapshot.json")?;
        let targets: Signed<Targets> = self.read("targets.json")?;
        root.signed
            .verify_role(&timestamp)
            .map_err(|_| signature())?;
        root.signed
            .verify_role(&snapshot)
            .map_err(|_| signature())?;
        if timestamp
            .signed
            .meta
            .get("snapshot.json")
            .map(|entry| entry.version)
            != Some(snapshot.signed.version)
            || snapshot
                .signed
                .meta
                .get("targets.json")
                .map(|entry| entry.version)
                != Some(targets.signed.version)
        {
            return Err(signature());
        }
        let time: jiff::Timestamp = self.read("latest_known_time.json")?;
        let now = jiff::Timestamp::now();
        if now < time
            || [
                root.signed.expires,
                timestamp.signed.expires,
                snapshot.signed.expires,
                targets.signed.expires,
            ]
            .into_iter()
            .any(|expires| expires <= now)
        {
            return Err(signature());
        }
        Ok(Proof { root, targets })
    }
}

#[derive(Debug, Clone)]
struct MetadataTransport(PathBuf);

impl MetadataTransport {
    fn metadata_filename(url: &Url) -> Option<&str> {
        if url.scheme() != "file" || url.host_str().is_some() {
            return None;
        }
        let name = url.path().strip_prefix("/numera-metadata/")?;
        if matches!(name, "timestamp.json" | "snapshot.json" | "targets.json") {
            return Some(name);
        }
        let (version, role) = name.split_once('.')?;
        if version.len() <= 20
            && version.parse::<std::num::NonZeroU64>().is_ok()
            && version.bytes().all(|byte| byte.is_ascii_digit())
            && matches!(role, "root.json" | "snapshot.json" | "targets.json")
        {
            return Some(name);
        }
        None
    }
}

#[async_trait::async_trait]
impl Transport for MetadataTransport {
    async fn fetch(&self, url: Url) -> Result<TransportStream, TransportError> {
        let name = Self::metadata_filename(&url).ok_or_else(|| {
            TransportError::new(TransportErrorKind::UnsupportedUrlScheme, url.clone())
        })?;
        match bounded_read(&self.0.join(name), MAX_METADATA_BYTES) {
            Ok(data) if strict_json(&data).is_ok() => {
                Ok(Box::pin(futures::stream::once(async { Ok(data.into()) })))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Err(TransportError::new(TransportErrorKind::FileNotFound, url))
            }
            _ => Err(TransportError::new(TransportErrorKind::Other, url)),
        }
    }
}

pub(super) struct OfflineRepository {
    anchor: Vec<u8>,
    metadata: PathBuf,
    pub(super) targets: PathBuf,
}

impl OfflineRepository {
    pub(super) fn open(trusted_root: &Path, local_repo: &Path) -> Result<Self, CoreError> {
        let repo = absolute_directory(local_repo).map_err(|_| signature())?;
        let root = std::path::absolute(trusted_root).map_err(|_| signature())?;
        absolute_directory(root.parent().ok_or_else(signature)?).map_err(|_| signature())?;
        let anchor = bounded_read(&root, MAX_METADATA_BYTES).map_err(|_| signature())?;
        let canonical_root = fs::canonicalize(&root).map_err(|_| signature())?;
        let canonical_repo = fs::canonicalize(&repo).map_err(|_| signature())?;
        if canonical_root.starts_with(canonical_repo) || strict_json(&anchor).is_err() {
            return Err(signature());
        }
        let provisioned: Signed<Root> = parse(&anchor).map_err(|_| signature())?;
        provisioned
            .signed
            .verify_role(&provisioned)
            .map_err(|_| signature())?;
        let metadata = repo.join("metadata");
        let targets = repo.join("targets");
        directory(&metadata).map_err(|_| signature())?;
        Ok(Self {
            anchor,
            metadata,
            targets,
        })
    }

    pub(super) fn refresh(
        &self,
        prior: Option<&Trust>,
        scratch: &Path,
    ) -> Result<(Trust, Result<Proof, CoreError>), CoreError> {
        let anchor = sha256(&self.anchor);
        if prior.is_some_and(|prior| anchor != prior.anchor) {
            return Err(signature());
        }
        let temp = tempfile::tempdir_in(scratch).map_err(storage_error)?;
        if let Some(prior) = prior {
            for (name, data) in &prior.roles {
                if !CACHE.contains(&name.as_str()) || data.len() as u64 > MAX_METADATA_BYTES {
                    return Err(recovery());
                }
                fs::write(temp.path().join(name), data).map_err(storage_error)?;
            }
        }
        let starting_root = prior
            .and_then(|prior| prior.roles.get("root.json"))
            .map_or(self.anchor.as_slice(), String::as_bytes);
        let bootstrap: Signed<Root> = parse(starting_root).map_err(|_| signature())?;
        if bootstrap.signed.version.get() > u64::MAX - MAX_ROOT_UPDATES {
            return Err(signature());
        }
        let metadata_url = Url::parse("file:///numera-metadata/").map_err(|_| recovery())?;
        let targets_url = Url::parse("file:///numera-targets/").map_err(|_| recovery())?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .map_err(storage_error)?;
        let loaded = runtime.block_on(
            RepositoryLoader::new(&starting_root, metadata_url, targets_url)
                .transport(MetadataTransport(self.metadata.clone()))
                .limits(Limits {
                    max_root_size: MAX_METADATA_BYTES,
                    max_timestamp_size: MAX_METADATA_BYTES,
                    max_snapshot_size: MAX_METADATA_BYTES,
                    max_targets_size: MAX_METADATA_BYTES,
                    max_root_updates: MAX_ROOT_UPDATES,
                })
                .datastore(temp.path())
                .load(),
        );
        let mut roles = BTreeMap::new();
        for name in CACHE {
            let data = match bounded_read(&temp.path().join(name), MAX_METADATA_BYTES) {
                Ok(data) => String::from_utf8(data).map_err(|_| recovery())?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(storage_error(error)),
            };
            roles.insert(name.to_owned(), data);
        }
        let trust = Trust {
            anchor,
            roles,
            complete: loaded.is_ok(),
        };
        if let Some(prior) = prior {
            let before = prior
                .roles
                .get("root.json")
                .map(|role| parse::<Signed<Root>>(role.as_bytes()))
                .transpose()?;
            let after = trust
                .roles
                .get("root.json")
                .map(|role| parse::<Signed<Root>>(role.as_bytes()))
                .transpose()?;
            if before == after {
                check_immutable_version::<Timestamp>(prior, &trust, "timestamp.json")?;
                check_immutable_version::<Snapshot>(prior, &trust, "snapshot.json")?;
                check_immutable_version::<Targets>(prior, &trust, "targets.json")?;
            }
        }
        // The scratch cache contains only role writes accepted by TUF. Commit
        // those floors even if a later role in this repository failed to load.
        let proof = loaded
            .map(|repository| Proof {
                root: repository.root().clone(),
                targets: repository.targets().clone(),
            })
            .map_err(|_| signature());
        Ok((trust, proof))
    }
}

fn check_immutable_version<T: Role + Serialize + serde::de::DeserializeOwned>(
    prior: &Trust,
    next: &Trust,
    role: &str,
) -> Result<(), CoreError> {
    if prior.roles.contains_key(role) && next.roles.contains_key(role) {
        let previous: Signed<T> = prior.read(role)?;
        let current: Signed<T> = next.read(role)?;
        if previous.signed.version() == current.signed.version()
            && previous.signed.canonical_form().map_err(|_| signature())?
                != current.signed.canonical_form().map_err(|_| signature())?
        {
            return Err(signature());
        }
    }
    Ok(())
}
