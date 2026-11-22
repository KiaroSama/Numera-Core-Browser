//! Local signed Chromium package admission and immutable digest-addressed storage.
mod payload;
mod storage;
mod trust;

pub use payload::{
    RuntimeTargetCustom, RuntimeTargetDescriptor, RuntimeTargetHashes, describe_runtime_archive,
    runtime_inventory_sha256,
};

use self::{
    payload::{Members, copy_archive, receipt_bytes, unpack, verify_installed, verify_inventory},
    storage::{
        absolute_directory, bounded_read, bytes, create_private_directory, directory, missing,
        parse, private_directory, publish, recovery, regular, sha256, signature, storage_error,
        sync_dir, write_state,
    },
    trust::{Authorized, OfflineRepository, Proof, Trust},
};
use crate::{EngineManifest, digest_string};
use core_contracts::{BuildPolicy, CoreError, EngineFamily, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

const STATE_LIMIT: u64 = 12 * 1024 * 1024;
const INVENTORY_LIMIT: usize = core_contracts::MAX_FRAME - 16 * 1024;
const STORE_LOCK: &str = "store.lock";
const LEASE: &str = "lease.lock";
const STATE: &str = "state.json";
const PACKAGES: &str = "packages";
const RECEIPT: &str = "receipt.json";
const RUNTIME: &str = "runtime";

#[derive(Debug, Clone, Serialize)]
pub struct PackageInfo {
    pub sha256: String,
    pub archive_bytes: u64,
    pub manifest: EngineManifest,
}

/// Holds a cross-process shared lease on the exact package for this launch.
/// Keep the lease until the complete owned browser process tree has exited.
#[derive(Debug)]
pub struct EngineLease {
    info: PackageInfo,
    executable: PathBuf,
    #[cfg(target_os = "linux")]
    sandbox_helper: Option<payload::Member>,
    _lease: File,
}

impl EngineLease {
    pub fn sha256(&self) -> &str {
        &self.info.sha256
    }

    pub fn manifest(&self) -> &EngineManifest {
        &self.info.manifest
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Verify an operator-installed copy against the selected signed package.
    #[cfg(target_os = "linux")]
    pub fn verify_sandbox_helper(&self, helper: &Path) -> Result<(), CoreError> {
        payload::verify_sandbox_helper(helper, self.sandbox_helper.as_ref())
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
    active: Option<String>,
    previous: Option<String>,
    records: BTreeMap<String, String>,
    trust: Option<Trust>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    proof: Proof,
    members: Members,
}

/// The root is an operator-selected location under the current user's control.
/// All published package directories are immutable. This store has no automatic
/// reclamation because a manager or a recoverable session may still have a pin.
#[derive(Debug, Clone)]
pub struct EngineStore {
    path: PathBuf,
}

impl EngineStore {
    /// Select a store without creating it or changing its state.
    pub fn open(path: &Path) -> Result<Self, CoreError> {
        trust::platform()?;
        let path = absolute_directory(path).map_err(storage_error)?;
        match private_directory(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(storage_error(error)),
        }
        Ok(Self { path })
    }

    fn admin(&self) -> Result<File, CoreError> {
        absolute_directory(&self.path).map_err(storage_error)?;
        create_private_directory(&self.path).map_err(storage_error)?;
        let lock = regular(&self.path.join(STORE_LOCK), true).map_err(storage_error)?;
        lock.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => CoreError::new(
                ErrorCode::OwnershipConflict,
                "Another process is updating the engine store",
            ),
            std::fs::TryLockError::Error(error) => storage_error(error),
        })?;
        create_private_directory(&self.path.join(PACKAGES)).map_err(storage_error)?;
        sync_dir(&self.path).map_err(storage_error)?;
        if let Some(parent) = self.path.parent() {
            sync_dir(parent).map_err(storage_error)?;
        }
        Ok(lock)
    }

    fn state(&self) -> Result<State, CoreError> {
        absolute_directory(&self.path).map_err(storage_error)?;
        match private_directory(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(State::default()),
            Err(error) => return Err(storage_error(error)),
        }
        let state: State = match bounded_read(&self.path.join(STATE), STATE_LIMIT) {
            Ok(data) => parse(&data)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match fs::read_dir(self.path.join(PACKAGES)) {
                    Ok(mut packages) => {
                        if packages.next().is_none() {
                            return Ok(State::default());
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        return Ok(State::default());
                    }
                    Err(_) => {}
                }
                return Err(recovery());
            }
            Err(error) => return Err(storage_error(error)),
        };
        if !state
            .records
            .iter()
            .all(|(key, digest)| is_normal_digest(key) && is_normal_digest(digest))
            || state
                .active
                .iter()
                .chain(&state.previous)
                .any(|digest| !state.records.contains_key(digest))
            || (state.previous.is_some() && state.previous == state.active)
            || (!state.records.is_empty() && state.trust.is_none())
        {
            return Err(recovery());
        }
        Ok(state)
    }

    fn commit(&self, state: &State) -> Result<(), CoreError> {
        let data = bytes(state)?;
        if data.len() as u64 > STATE_LIMIT {
            return Err(CoreError::new(
                ErrorCode::InsufficientResources,
                "The engine store state exceeds its supported size",
            ));
        }
        write_state(&self.path.join(STATE), &data)
    }

    fn selection(
        &self,
        family: &EngineFamily,
        policy: &BuildPolicy,
        state: &State,
    ) -> Result<String, CoreError> {
        if family != &EngineFamily::Chromium {
            return Err(CoreError::new(
                ErrorCode::UnsupportedCapability,
                "Only Chromium packages are supported",
            ));
        }
        match policy {
            BuildPolicy::Approved => state.active.clone().ok_or_else(missing),
            BuildPolicy::Pinned { sha256 } => normalize_digest(sha256),
        }
    }

    fn inspect_receipt(
        &self,
        dir: &Path,
        digest: &str,
        checksum: Option<&str>,
    ) -> Result<(Authorized, Members, String), CoreError> {
        directory(&self.path.join(PACKAGES)).map_err(storage_error)?;
        directory(dir).map_err(storage_error)?;
        let data = receipt_bytes(&dir.join(RECEIPT))?;
        let hash = sha256(&data);
        if checksum.is_some_and(|checksum| checksum != hash) {
            return Err(recovery());
        }
        let receipt: Receipt = parse(&data)?;
        let authorized = receipt.proof.authorized()?;
        if digest != authorized.package.sha256 {
            return Err(signature());
        }
        verify_inventory(
            &receipt.members,
            &authorized.package.manifest,
            &authorized.inventory,
        )?;
        Ok((authorized, receipt.members, hash))
    }

    fn recorded(
        &self,
        state: &State,
        digest: &str,
    ) -> Result<(Authorized, Members, PathBuf), CoreError> {
        let checksum = state.records.get(digest).ok_or_else(missing)?;
        let dir = self.path.join(PACKAGES).join(digest);
        let (authorized, inventory, _) = self.inspect_receipt(&dir, digest, Some(checksum))?;
        Ok((authorized, inventory, dir))
    }

    /// Authenticate the selected manifest without walking or hashing runtime files.
    pub fn select(
        &self,
        family: &EngineFamily,
        policy: &BuildPolicy,
    ) -> Result<PackageInfo, CoreError> {
        let state = self.state()?;
        let digest = self.selection(family, policy, &state)?;
        let (authorized, _, _) = self.recorded(&state, &digest)?;
        Ok(authorized.package)
    }

    /// Resolve and lease a fully verified Chromium runtime for execution.
    pub fn resolve(
        &self,
        family: &EngineFamily,
        policy: &BuildPolicy,
    ) -> Result<EngineLease, CoreError> {
        let state = self.state()?;
        let digest = self.selection(family, policy, &state)?;
        let (authorized, inventory, dir) = self.recorded(&state, &digest)?;
        let lease = regular(&dir.join(LEASE), false).map_err(storage_error)?;
        lease.try_lock_shared().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => CoreError::new(
                ErrorCode::OwnershipConflict,
                "The selected engine package is exclusively leased",
            ),
            std::fs::TryLockError::Error(error) => storage_error(error),
        })?;
        let runtime = dir.join(RUNTIME);
        verify_installed(&runtime, &inventory)?;
        let executable = runtime.join(&authorized.package.manifest.executable);
        #[cfg(target_os = "linux")]
        let sandbox_helper = {
            let relative = Path::new(&authorized.package.manifest.executable);
            // Chromium tries an adjacent chrome-sandbox before the configured path.
            if inventory.contains_key(
                relative
                    .with_file_name("chrome-sandbox")
                    .to_str()
                    .unwrap_or_default(),
            ) {
                None
            } else {
                inventory
                    .get(
                        relative
                            .with_file_name("chrome_sandbox")
                            .to_str()
                            .unwrap_or_default(),
                    )
                    .cloned()
            }
        };
        Ok(EngineLease {
            info: authorized.package,
            executable,
            #[cfg(target_os = "linux")]
            sandbox_helper,
            _lease: lease,
        })
    }

    /// Verify the admission signatures and all files of an installed package.
    pub fn verify(&self, sha256: &str) -> Result<PackageInfo, CoreError> {
        let lease = self.resolve(
            &EngineFamily::Chromium,
            &BuildPolicy::Pinned {
                sha256: sha256.to_owned(),
            },
        )?;
        Ok(lease.info)
    }

    pub fn inventory(&self) -> Result<Value, CoreError> {
        let state = self.state()?;
        let available = state
            .trust
            .as_ref()
            .and_then(|trust| trust.current().ok())
            .and_then(|proof| proof.authorized().ok())
            .map(|allowed| allowed.package);
        let mut result = crate::inventory();
        result["available_approved"] = json!(available.into_iter().collect::<Vec<_>>());
        let mut used = bytes(&result)?.len();
        if used > INVENTORY_LIMIT {
            return Err(inventory_overflow());
        }
        let mut installed = Vec::new();
        for digest in state.records.keys() {
            let (authorized, _, _) = self.recorded(&state, digest)?;
            let entry = json!({
                "sha256": authorized.package.sha256,
                "archive_bytes": authorized.package.archive_bytes,
                "manifest": authorized.package.manifest,
                "active": state.active.as_ref() == Some(digest),
                "previous": state.previous.as_ref() == Some(digest),
            });
            used += bytes(&entry)?.len() + 1;
            if used > INVENTORY_LIMIT {
                return Err(inventory_overflow());
            }
            installed.push(entry);
        }
        result["installed"] = json!(installed);
        if state.active.is_some() {
            result["families"][0]["status"] = json!("installed");
            result["families"][0]["reason"] = json!("An authenticated Chromium package is active");
        }
        Ok(result)
    }

    fn refresh(
        &self,
        state: &mut State,
        repository: &OfflineRepository,
    ) -> Result<Proof, CoreError> {
        let (trust, result) = repository.refresh(state.trust.as_ref(), &self.path)?;
        state.trust = Some(trust);
        // Persist rollback floors before a candidate can be returned or copied.
        self.commit(state)?;
        result
    }

    /// Verify the offline repository and persist its authenticated rollback floors.
    pub fn check(&self, trusted_root: &Path, repository: &Path) -> Result<PackageInfo, CoreError> {
        let repository = OfflineRepository::open(trusted_root, repository)?;
        let _lock = self.admin()?;
        let mut state = self.state()?;
        Ok(self.refresh(&mut state, &repository)?.authorized()?.package)
    }

    fn stage_with_lock(
        &self,
        state: &mut State,
        repository: &OfflineRepository,
    ) -> Result<PackageInfo, CoreError> {
        let proof = self.refresh(state, repository)?;
        let allowed = proof.authorized()?;
        let digest = &allowed.package.sha256;
        let packages = self.path.join(PACKAGES);
        let dir = packages.join(digest);
        let checksum = match fs::symlink_metadata(&dir) {
            // An interrupted transaction can leave a complete immutable directory
            // before it has a state record. Reuse it only after full verification.
            Ok(_) => {
                let (previous, members, checksum) = self.inspect_receipt(
                    &dir,
                    digest,
                    state.records.get(digest).map(String::as_str),
                )?;
                compare_authorizations(&allowed, &previous)?;
                verify_installed(&dir.join(RUNTIME), &members)?;
                checksum
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if state.records.contains_key(digest) {
                    return Err(recovery());
                }
                directory(&repository.targets).map_err(|_| signature())?;
                let scratch = tempfile::tempdir_in(&packages).map_err(storage_error)?;
                let archive = copy_archive(
                    &repository.targets.join(proof.archive_name(digest)?),
                    scratch.path(),
                    digest,
                    allowed.package.archive_bytes,
                )?;
                let members = unpack(archive, digest, &scratch.path().join(RUNTIME))?;
                verify_inventory(&members, &allowed.package.manifest, &allowed.inventory)?;
                let data = bytes(&Receipt { proof, members })?;
                write_state(&scratch.path().join(RECEIPT), &data)?;
                regular(&scratch.path().join(LEASE), true)
                    .map_err(storage_error)?
                    .sync_all()
                    .map_err(storage_error)?;
                sync_dir(scratch.path()).map_err(storage_error)?;
                publish(scratch.path(), &dir, false).map_err(storage_error)?;
                sync_dir(&packages).map_err(storage_error)?;
                sha256(&data)
            }
            Err(error) => return Err(storage_error(error)),
        };
        state.records.insert(digest.clone(), checksum);
        self.commit(state)?;
        Ok(allowed.package)
    }

    /// Verify, extract, and retain an immutable package without setting the default.
    pub fn stage(&self, trusted_root: &Path, repository: &Path) -> Result<PackageInfo, CoreError> {
        let repository = OfflineRepository::open(trusted_root, repository)?;
        let _lock = self.admin()?;
        self.stage_with_lock(&mut self.state()?, &repository)
    }

    fn activate_with_lock(
        &self,
        state: &mut State,
        digest: &str,
    ) -> Result<PackageInfo, CoreError> {
        let (requested, inventory, dir) = self.recorded(state, digest)?;
        if state.active.as_deref() != Some(digest) {
            let current = state
                .trust
                .as_ref()
                .ok_or_else(signature)?
                .current()?
                .authorized()?;
            compare_authorizations(&current, &requested)?;
            verify_installed(&dir.join(RUNTIME), &inventory)?;
            state.previous = state.active.replace(digest.to_owned());
            self.commit(state)?;
        } else {
            verify_installed(&dir.join(RUNTIME), &inventory)?;
        }
        Ok(requested.package)
    }

    /// Set the default for new unpinned sessions to a currently authorized digest.
    pub fn activate(&self, sha256: &str) -> Result<PackageInfo, CoreError> {
        let digest = normalize_digest(sha256)?;
        let _lock = self.admin()?;
        self.activate_with_lock(&mut self.state()?, &digest)
    }

    /// Stage and activate an offline package under the same administrative lock.
    pub fn install(
        &self,
        trusted_root: &Path,
        repository: &Path,
    ) -> Result<PackageInfo, CoreError> {
        let repository = OfflineRepository::open(trusted_root, repository)?;
        let _lock = self.admin()?;
        let mut state = self.state()?;
        let staged = self.stage_with_lock(&mut state, &repository)?;
        self.activate_with_lock(&mut state, &staged.sha256)
    }

    /// Restore the prior default. Profile directories are never modified by the store.
    pub fn rollback(&self) -> Result<PackageInfo, CoreError> {
        let _lock = self.admin()?;
        let mut state = self.state()?;
        let prior = state.previous.as_deref().ok_or_else(missing)?;
        let (prior, inventory, dir) = self.recorded(&state, prior)?;
        let active = state.active.as_deref().ok_or_else(recovery)?;
        let (active, _, _) = self.recorded(&state, active)?;
        if active.package.manifest.profile_data_format != prior.package.manifest.profile_data_format
        {
            return Err(CoreError::new(
                ErrorCode::IncompatibleEngineDataFormat,
                "Rollback would change the profile data format",
            ));
        }
        verify_installed(&dir.join(RUNTIME), &inventory)?;
        std::mem::swap(&mut state.active, &mut state.previous);
        self.commit(&state)?;
        Ok(prior.package)
    }
}

fn is_normal_digest(digest: &str) -> bool {
    digest_string(digest) && !digest.bytes().any(|byte| byte.is_ascii_uppercase())
}

fn inventory_overflow() -> CoreError {
    CoreError::new(
        ErrorCode::InsufficientResources,
        "The engine inventory exceeds the response size limit",
    )
}

fn normalize_digest(digest: &str) -> Result<String, CoreError> {
    if !digest_string(digest) {
        return Err(CoreError::invalid("Expected a SHA-256 package digest"));
    }
    Ok(digest.to_ascii_lowercase())
}

fn compare_authorizations(expected: &Authorized, actual: &Authorized) -> Result<(), CoreError> {
    if expected.inventory != actual.inventory
        || bytes(&expected.package)? != bytes(&actual.package)?
    {
        return Err(signature());
    }
    Ok(())
}
