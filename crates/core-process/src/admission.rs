use core_contracts::{CoreError, ErrorCode, MAX_FRAME, strict_json, valid_id};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Lifecycle {
    Admitted,
    Starting,
    Running,
    Stopping,
    Stopped,
    RecoveryRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claim {
    pub schema_revision: u16,
    pub profile_id: String,
    pub launch_id: String,
    pub idempotency_key: String,
    pub owner_nonce: String,
    pub supervisor_pid: u32,
    pub browser_pid: Option<u32>,
    pub sequence: u64,
    pub state: Lifecycle,
}

fn recovery() -> CoreError {
    CoreError::new(
        ErrorCode::RecoveryRequired,
        "Ownership state is ambiguous; no profile mutation or browser spawn is permitted",
    )
}

pub(crate) fn nonce() -> Result<String, CoreError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| {
        CoreError::new(
            ErrorCode::InsufficientResources,
            "OS random source unavailable",
        )
    })?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn private_directory(path: &Path) -> Result<(), CoreError> {
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
    };
    #[cfg(not(unix))]
    let builder = fs::DirBuilder::new();
    match builder.create(path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(_) => return Err(recovery()),
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| recovery())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(recovery());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != nix::unistd::geteuid().as_raw() || metadata.mode() & 0o077 != 0 {
            return Err(recovery());
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(recovery());
        }
    }
    Ok(())
}

fn nofollow(path: &Path, write: bool, create: bool, exclusive: bool) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(write)
        .create(create && !exclusive)
        .create_new(exclusive);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::other("Regular ownership file required"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 || metadata.uid() != nix::unistd::geteuid().as_raw() {
            return Err(std::io::Error::other("Ownership file identity mismatch"));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
        {
            return Err(std::io::Error::other("Reparse ownership file rejected"));
        }
    }
    Ok(file)
}

fn read_claim(path: &Path, profile: &str) -> Result<Option<Claim>, CoreError> {
    let file = match nofollow(path, false, false, false) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(recovery()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FRAME as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| recovery())?;
    if bytes.len() > MAX_FRAME {
        return Err(recovery());
    }
    let claim: Claim = serde_json::from_value(strict_json(&bytes).map_err(|_| recovery())?)
        .map_err(|_| recovery())?;
    if claim.schema_revision != 1
        || claim.profile_id != profile
        || !valid_id(&claim.launch_id)
        || !valid_id(&claim.idempotency_key)
        || claim.owner_nonce.len() != 64
        || !claim.owner_nonce.bytes().all(|b| b.is_ascii_hexdigit())
        || claim.supervisor_pid == 0
    {
        return Err(recovery());
    }
    Ok(Some(claim))
}

/// A standalone authority only. Manager-owned grants must use their own bridge;
/// this API must not acquire a second copy of a manager's non-reentrant lease.
pub struct StandaloneLease {
    directory: PathBuf,
    claim_path: PathBuf,
    claim: Claim,
    _lock: File,
}

impl StandaloneLease {
    pub fn acquire(
        data_root: &Path,
        profile: &str,
        launch: &str,
        idempotency: &str,
    ) -> Result<Self, CoreError> {
        if !data_root.is_absolute() || ![profile, launch, idempotency].into_iter().all(valid_id) {
            return Err(CoreError::invalid(
                "Invalid standalone ownership root or identifier",
            ));
        }
        // The explicit data root must already exist and be owned by the caller.
        let metadata = fs::symlink_metadata(data_root).map_err(|_| recovery())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(recovery());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.uid() != nix::unistd::geteuid().as_raw() || metadata.mode() & 0o022 != 0 {
                return Err(recovery());
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes()
                & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                != 0
            {
                return Err(recovery());
            }
        }
        let directory = data_root
            .canonicalize()
            .map_err(|_| recovery())?
            .join("core-ownership");
        private_directory(&directory)?;
        let lock = nofollow(
            &directory.join(format!("{profile}.lock")),
            true,
            true,
            false,
        )
        .map_err(|_| recovery())?;
        match lock.try_lock() {
            Ok(()) => (),
            Err(TryLockError::WouldBlock) => {
                return Err(CoreError::new(
                    ErrorCode::OwnershipConflict,
                    "Profile already has an admission holder",
                ));
            }
            Err(TryLockError::Error(_)) => return Err(recovery()),
        }
        let claim_path = directory.join(format!("{profile}.json"));
        let previous = read_claim(&claim_path, profile)?;
        if previous
            .as_ref()
            .is_some_and(|c| c.state != Lifecycle::Stopped)
        {
            return Err(recovery());
        }
        let claim = Claim {
            schema_revision: 1,
            profile_id: profile.into(),
            launch_id: launch.into(),
            idempotency_key: idempotency.into(),
            owner_nonce: nonce()?,
            supervisor_pid: std::process::id(),
            browser_pid: None,
            sequence: 0,
            state: Lifecycle::Admitted,
        };
        let lease = Self {
            directory,
            claim_path,
            claim,
            _lock: lock,
        };
        lease.persist(previous.as_ref())?;
        Ok(lease)
    }

    pub fn claim(&self) -> &Claim {
        &self.claim
    }

    fn persist(&self, expected: Option<&Claim>) -> Result<(), CoreError> {
        if read_claim(&self.claim_path, &self.claim.profile_id)?.as_ref() != expected {
            return Err(recovery());
        }
        let temporary = self.directory.join(format!(".{}.tmp", nonce()?));
        let result = (|| {
            let mut file = nofollow(&temporary, true, false, true).map_err(|_| recovery())?;
            let bytes = serde_json::to_vec(&self.claim).map_err(|_| recovery())?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| recovery())?;
            drop(file);
            fs::rename(&temporary, &self.claim_path).map_err(|_| recovery())?;
            #[cfg(unix)]
            File::open(&self.directory)
                .and_then(|dir| dir.sync_all())
                .map_err(|_| recovery())?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub(crate) fn transition(
        &mut self,
        next: Lifecycle,
        browser_pid: Option<u32>,
    ) -> Result<(), CoreError> {
        use Lifecycle::*;
        let valid = matches!(
            (self.claim.state, next),
            (Admitted, Starting | Stopped)
                | (Starting, Running | Stopping | RecoveryRequired)
                | (Running, Stopping | RecoveryRequired)
                | (Stopping, Stopped | RecoveryRequired)
                | (RecoveryRequired, Stopping)
        );
        if !valid || browser_pid == Some(0) {
            return Err(recovery());
        }
        let previous = self.claim.clone();
        self.claim.state = next;
        self.claim.sequence = self.claim.sequence.checked_add(1).ok_or_else(recovery)?;
        if browser_pid.is_some() {
            self.claim.browser_pid = browser_pid;
        }
        if let Err(error) = self.persist(Some(&previous)) {
            self.claim = previous;
            return Err(error);
        }
        Ok(())
    }

    /// Cancel an admission before spawn. A running session cannot use this to
    /// release ownership while descendants still exist.
    pub fn cancel(mut self) -> Result<(), CoreError> {
        if self.claim.state != Lifecycle::Admitted {
            return Err(recovery());
        }
        self.transition(Lifecycle::Stopped, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    struct Root(PathBuf);
    impl Root {
        fn new() -> Self {
            let p =
                std::env::temp_dir().join(format!("Numera فارسی ownership {}", nonce().unwrap()));
            private_directory(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn same_profile_conflicts_different_profiles_are_independent() {
        let r = Root::new();
        let a = StandaloneLease::acquire(&r.0, "a", "launch-a", "key-a").unwrap();
        assert_eq!(
            StandaloneLease::acquire(&r.0, "a", "retry", "key-a")
                .err()
                .unwrap()
                .code,
            ErrorCode::OwnershipConflict
        );
        let b = StandaloneLease::acquire(&r.0, "b", "launch-b", "key-b").unwrap();
        a.cancel().unwrap();
        b.cancel().unwrap();
        StandaloneLease::acquire(&r.0, "a", "next", "key-next")
            .unwrap()
            .cancel()
            .unwrap();
    }

    #[test]
    fn dropped_live_claim_requires_recovery_instead_of_pid_based_relaunch() {
        let r = Root::new();
        let a = StandaloneLease::acquire(&r.0, "a", "launch", "key").unwrap();
        drop(a);
        assert_eq!(
            StandaloneLease::acquire(&r.0, "a", "retry", "key")
                .err()
                .unwrap()
                .code,
            ErrorCode::RecoveryRequired
        );
    }

    #[test]
    fn tampered_owner_and_invalid_state_cannot_be_released() {
        let r = Root::new();
        let mut a = StandaloneLease::acquire(&r.0, "a", "launch", "key").unwrap();
        assert!(a.transition(Lifecycle::Running, Some(1)).is_err());
        let p = r.0.join("core-ownership/a.json");
        fs::write(&p, b"{\"bad\":true}").unwrap();
        assert!(a.cancel().is_err());
        assert_eq!(fs::read(p).unwrap(), b"{\"bad\":true}");
    }

    #[test]
    fn corrupt_or_duplicate_claims_fail_closed_without_rewriting() {
        let r = Root::new();
        private_directory(&r.0.join("core-ownership")).unwrap();
        let p = r.0.join("core-ownership/a.json");
        let bad = b"{\"schema_revision\":1,\"schema_revision\":2}";
        fs::write(&p, bad).unwrap();
        assert!(StandaloneLease::acquire(&r.0, "a", "launch", "key").is_err());
        assert_eq!(fs::read(p).unwrap(), bad);
    }

    #[test]
    fn oversized_claim_and_same_nonce_state_rewrite_are_preserved() {
        let r = Root::new();
        let a = StandaloneLease::acquire(&r.0, "a", "launch", "key").unwrap();
        let p = r.0.join("core-ownership/a.json");
        let mut edited = a.claim().clone();
        edited.state = Lifecycle::Stopped;
        fs::write(&p, serde_json::to_vec(&edited).unwrap()).unwrap();
        assert!(a.cancel().is_err());
        assert_eq!(read_claim(&p, "a").unwrap().unwrap(), edited);
        let mut oversized = serde_json::to_vec(&edited).unwrap();
        oversized.resize(MAX_FRAME, b' ');
        oversized.extend_from_slice(b"ambiguous trailing data");
        fs::write(&p, &oversized).unwrap();
        assert!(StandaloneLease::acquire(&r.0, "a", "next", "next").is_err());
        assert_eq!(fs::read(p).unwrap(), oversized);
    }

    #[test]
    fn subprocess_lease_holder() {
        if std::env::var("NUMERA_FIXTURE_ROLE").as_deref() != Ok("lease-holder") {
            return;
        }
        let root = PathBuf::from(std::env::var_os("NUMERA_FIXTURE_ROOT").unwrap());
        let lease = StandaloneLease::acquire(&root, "cross-process", "launch", "key").unwrap();
        fs::write(root.join("ready"), b"ready").unwrap();
        let started = Instant::now();
        while !root.join("finish").exists() {
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "Test lease helper timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        lease.cancel().unwrap();
    }

    #[test]
    fn actual_cross_process_lease_is_exclusive_and_survives_lost_client_reply() {
        let r = Root::new();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "admission::tests::subprocess_lease_holder"])
            .env("NUMERA_FIXTURE_ROLE", "lease-holder")
            .env("NUMERA_FIXTURE_ROOT", &r.0)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let started = Instant::now();
        while !r.0.join("ready").exists() {
            if started.elapsed() > Duration::from_secs(10) {
                let _ = child.kill();
                panic!("Lease helper failed to become ready");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let code = StandaloneLease::acquire(&r.0, "cross-process", "retry", "key")
            .err()
            .unwrap()
            .code;
        fs::write(r.0.join("finish"), b"finish").unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(code, ErrorCode::OwnershipConflict);
        StandaloneLease::acquire(&r.0, "cross-process", "next", "next-key")
            .unwrap()
            .cancel()
            .unwrap();
    }

    #[test]
    fn killed_holder_does_not_authorize_a_second_launch() {
        let r = Root::new();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "admission::tests::subprocess_lease_holder"])
            .env("NUMERA_FIXTURE_ROLE", "lease-holder")
            .env("NUMERA_FIXTURE_ROOT", &r.0)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let started = Instant::now();
        while !r.0.join("ready").exists() {
            if started.elapsed() > Duration::from_secs(10) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Lease helper failed to become ready");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        let path = r.0.join("core-ownership/cross-process.json");
        let before = fs::read(&path).unwrap();
        assert_eq!(
            StandaloneLease::acquire(&r.0, "cross-process", "retry", "key")
                .err()
                .unwrap()
                .code,
            ErrorCode::RecoveryRequired
        );
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_or_hardlink_ownership_files_are_rejected() {
        use std::os::unix::fs::symlink;
        let r = Root::new();
        private_directory(&r.0.join("core-ownership")).unwrap();
        let outside = r.0.join("target");
        fs::write(&outside, b"preserve").unwrap();
        let lock = r.0.join("core-ownership/a.lock");
        symlink(&outside, &lock).unwrap();
        assert!(StandaloneLease::acquire(&r.0, "a", "launch", "key").is_err());
        fs::remove_file(&lock).unwrap();
        fs::hard_link(&outside, &lock).unwrap();
        assert!(StandaloneLease::acquire(&r.0, "a", "launch", "key").is_err());
        assert_eq!(fs::read(outside).unwrap(), b"preserve");
    }
}
