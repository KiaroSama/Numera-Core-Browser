use crate::admission::{Lifecycle, StandaloneLease, nofollow, nonce, private_directory};
#[cfg(target_os = "linux")]
use crate::linux::OwnedProcessTree;
#[cfg(windows)]
use crate::windows::OwnedProcessTree;
use core_contracts::{CoreError, ErrorCode, LaunchSnapshot};
use sha2::{Digest, Sha256};
#[cfg(windows)]
use std::time::Duration;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    io::{ErrorKind, Read, Write},
    path::{Component, Path, PathBuf},
    time::Instant,
};

pub struct SessionPaths {
    root: PathBuf,
    profile: PathBuf,
    cache: PathBuf,
}

fn recovery(message: &'static str) -> CoreError {
    CoreError::new(ErrorCode::RecoveryRequired, message)
}

fn relative(root: &Path, path: &str) -> Result<PathBuf, CoreError> {
    let child = Path::new(path).strip_prefix(root).map_err(|_| {
        CoreError::invalid("Profile and cache directories must be inside the configured data root")
    })?;
    if child.as_os_str().is_empty()
        || child.components().any(|part| match part {
            Component::Normal(name) => name.to_str().is_none_or(|name| {
                name.ends_with(['.', ' '])
                    || name
                        .chars()
                        .any(|ch| ch.is_ascii_control() || "<>:\"\\|?*".contains(ch))
            }),
            _ => true,
        })
        || child
            .components()
            .next()
            .is_some_and(|part| part.as_os_str().eq_ignore_ascii_case("core-ownership"))
    {
        return Err(CoreError::invalid(
            "Invalid profile or cache directory grant",
        ));
    }
    Ok(child.to_owned())
}

impl SessionPaths {
    pub fn validate(root: &Path, snapshot: &LaunchSnapshot) -> Result<Self, CoreError> {
        if !root.is_absolute() || Path::new(&snapshot.resources.data_root) != root {
            return Err(CoreError::invalid(
                "Snapshot data root must match the configured root",
            ));
        }
        let profile = relative(root, &snapshot.resources.native_profile_dir)?;
        let cache = relative(root, &snapshot.resources.cache_root)?;
        if profile.starts_with(&cache) || cache.starts_with(&profile) {
            return Err(CoreError::invalid(
                "Profile and cache directories must be separate",
            ));
        }
        Ok(Self {
            root: root.to_owned(),
            profile,
            cache: cache.join(identifier(&snapshot.profile_id)),
        })
    }

    fn prepare(mut self) -> Result<Self, CoreError> {
        self.root = existing_root(&self.root)?;
        for relative in [&mut self.profile, &mut self.cache] {
            let mut target = self.root.clone();
            for part in relative.components() {
                target.push(part);
                private_directory(&target)?;
            }
            *relative = target
                .canonicalize()
                .map_err(|_| recovery("Cannot resolve native session directory"))?;
        }
        if self.profile.starts_with(&self.cache) || self.cache.starts_with(&self.profile) {
            return Err(CoreError::invalid(
                "Profile and cache grants resolve to overlapping directories",
            ));
        }
        Ok(self)
    }

    pub fn profile(&self) -> &Path {
        &self.profile
    }
    pub fn cache(&self) -> &Path {
        &self.cache
    }
}

fn existing_root(root: &Path) -> Result<PathBuf, CoreError> {
    if !root.is_absolute()
        || root
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(CoreError::invalid(
            "Expected an absolute data root without traversal",
        ));
    }
    let mut ancestor = PathBuf::new();
    for part in root.components() {
        ancestor.push(part);
        if !matches!(part, Component::Prefix(_)) {
            reject_unsafe_directory(&ancestor)?;
        }
    }
    private_directory(root)?;
    root.canonicalize()
        .map_err(|_| recovery("Cannot resolve configured data directory"))
}

fn identifier(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn record_intent(root: &Path, snapshot: &LaunchSnapshot) -> Result<(), CoreError> {
    let journal = root.join("core-ownership").join("launches");
    private_directory(&journal)?;
    // Exclusive creation reserves both IDs across restarted or concurrent
    // control connections. A crash or conflicting retry cannot reuse a slot.
    for (kind, value) in [
        ("launch", &snapshot.launch_id),
        ("retry", &snapshot.idempotency_key),
    ] {
        let path = journal.join(format!("{kind}-{}", identifier(value)));
        let file = nofollow(&path, true, false, true).map_err(|error| {
            if error.kind() == ErrorKind::AlreadyExists {
                CoreError::new(
                    ErrorCode::OwnershipConflict,
                    "Launch or idempotency key was previously admitted",
                )
            } else {
                recovery("Cannot preserve launch idempotency")
            }
        })?;
        file.sync_all()
            .map_err(|_| recovery("Cannot persist launch idempotency"))?;
    }
    #[cfg(unix)]
    fs::File::open(journal)
        .and_then(|dir| dir.sync_all())
        .map_err(|_| recovery("Cannot persist launch journal"))?;
    Ok(())
}

fn reject_unsafe_directory(path: &Path) -> Result<(), CoreError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| recovery("Session data root or an ancestor is missing"))?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(recovery(
                "Reparse points are prohibited in the session data root",
            ));
        }
    }
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(recovery("Links are prohibited in the session data root"));
    }
    Ok(())
}

fn bind_profile(profile: &Path, digest: &str) -> Result<(), CoreError> {
    let marker = profile.join(".numera-package-sha256");
    let format_error = || {
        CoreError::new(
            ErrorCode::IncompatibleEngineDataFormat,
            "Profile requires its original package or an explicitly migrated copy",
        )
    };
    match nofollow(&marker, false, false, false) {
        Ok(file) => {
            let mut saved = String::new();
            file.take(66)
                .read_to_string(&mut saved)
                .map_err(|_| format_error())?;
            if saved.strip_suffix('\n') != Some(digest) {
                return Err(format_error());
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let contents = fs::read_dir(profile).map_err(|_| format_error())?;
            for entry in contents {
                if entry.map_err(|_| format_error())?.file_name() != "core-ownership" {
                    return Err(format_error());
                }
            }
            let mut file = nofollow(&marker, true, false, true).map_err(|_| format_error())?;
            writeln!(file, "{digest}")
                .and_then(|()| file.sync_all())
                .map_err(|_| format_error())?;
            #[cfg(unix)]
            fs::File::open(profile)
                .and_then(|dir| dir.sync_all())
                .map_err(|_| format_error())?;
        }
        Err(_) => return Err(format_error()),
    }
    Ok(())
}

pub fn profile_package(data_root: &Path, path: &str) -> Result<Option<String>, CoreError> {
    let relative = relative(data_root, path)?;
    let mut profile = existing_root(data_root)?;
    for part in relative.components() {
        profile.push(part);
        match fs::symlink_metadata(&profile) {
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(recovery("Cannot inspect the profile path")),
            Ok(_) => reject_unsafe_directory(&profile)?,
        }
    }
    match nofollow(&profile.join(".numera-package-sha256"), false, false, false) {
        Ok(file) => {
            let mut saved = String::new();
            file.take(66)
                .read_to_string(&mut saved)
                .map_err(|_| recovery("Cannot read the profile package binding"))?;
            saved
                .strip_suffix('\n')
                .filter(|hash| {
                    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
                .map(|hash| Some(hash.to_owned()))
                .ok_or_else(|| recovery("Invalid profile package binding"))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(_) => Err(recovery("Cannot open the profile package binding")),
    }
}

fn copy_profile_files(source: &Path, destination: &Path) -> Result<(), CoreError> {
    let failed = || recovery("Cannot make a complete private profile snapshot");
    let mut directories = vec![(source.to_owned(), destination.to_owned())];
    while let Some((from, to)) = directories.pop() {
        reject_unsafe_directory(&from)?;
        for item in fs::read_dir(&from).map_err(|_| failed())? {
            let item = item.map_err(|_| failed())?;
            let name = item.file_name();
            if from == source
                && matches!(
                    name.to_str(),
                    Some("core-ownership" | ".numera-package-sha256" | "lockfile")
                )
            {
                continue;
            }
            let entry = item.path();
            let output = to.join(name);
            let kind = item.file_type().map_err(|_| failed())?;
            if kind.is_dir() {
                reject_unsafe_directory(&entry)?;
                private_directory(&output)?;
                directories.push((entry, output));
            } else if kind.is_file() {
                let mut input = nofollow(&entry, false, false, false).map_err(|_| failed())?;
                let mut file = nofollow(&output, true, false, true).map_err(|_| failed())?;
                std::io::copy(&mut input, &mut file)
                    .and_then(|_| file.sync_all())
                    .map_err(|_| failed())?;
            } else {
                return Err(recovery(
                    "Snapshot requires a stopped profile with regular files and directories",
                ));
            }
        }
        #[cfg(unix)]
        fs::File::open(&to)
            .and_then(|dir| dir.sync_all())
            .map_err(|_| failed())?;
    }
    Ok(())
}

pub fn snapshot_profile(
    data_root: &Path,
    source: &str,
    destination: &str,
) -> Result<String, CoreError> {
    let source = relative(data_root, source)?;
    let destination = relative(data_root, destination)?;
    let root = existing_root(data_root)?;
    let preserve = |relative: &Path| -> Result<PathBuf, CoreError> {
        let mut path = root.clone();
        for component in relative.components() {
            path.push(component);
            reject_unsafe_directory(&path)?;
            private_directory(&path)?;
        }
        path.canonicalize()
            .map_err(|_| recovery("Cannot resolve snapshot directory"))
    };
    let source = preserve(&source)?;
    let parent = preserve(destination.parent().unwrap_or(Path::new("")))?;
    let destination = parent.join(
        destination
            .file_name()
            .ok_or_else(|| CoreError::invalid("Missing snapshot directory name"))?,
    );
    if parent.starts_with(&source) || source.starts_with(&destination) {
        return Err(CoreError::invalid(
            "Source and snapshot directories must be separate",
        ));
    }
    let id = nonce()?;
    let from = StandaloneLease::acquire(&source, "profile", &id, &id)?;
    let result = (|| {
        #[cfg(windows)]
        profile_available(&source, Instant::now())?;
        let mut digest = String::new();
        nofollow(&source.join(".numera-package-sha256"), false, false, false)
            .and_then(|file| file.take(66).read_to_string(&mut digest))
            .map_err(|_| recovery("Snapshot source has no readable package binding"))?;
        let digest = digest
            .strip_suffix('\n')
            .filter(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| recovery("Snapshot source has an invalid package binding"))?;
        #[cfg(unix)]
        let created = std::os::unix::fs::DirBuilderExt::mode(&mut fs::DirBuilder::new(), 0o700)
            .create(&destination);
        #[cfg(windows)]
        let created = fs::create_dir(&destination);
        created.map_err(|_| CoreError::invalid("Snapshot destination must be a new directory"))?;
        let to = StandaloneLease::acquire(&destination, "profile", &id, &id)?;
        let copied = (|| {
            copy_profile_files(&source, &destination)?;
            let mut marker = nofollow(
                &destination.join(".numera-package-sha256"),
                true,
                false,
                true,
            )
            .map_err(|_| recovery("Cannot publish the snapshot package binding"))?;
            writeln!(marker, "{digest}")
                .and_then(|()| marker.sync_all())
                .map_err(|_| recovery("Cannot persist snapshot package binding"))?;
            #[cfg(unix)]
            fs::File::open(&destination)
                .and_then(|dir| dir.sync_all())
                .map_err(|_| recovery("Cannot persist snapshot directory"))?;
            Ok(digest.to_owned())
        })();
        // A failed copy remains inspectable. Without the binding it cannot launch.
        to.cancel()?;
        copied
    })();
    from.cancel()?;
    result
}

#[cfg(windows)]
fn profile_available(profile: &Path, deadline: Instant) -> Result<(), CoreError> {
    use std::os::windows::fs::OpenOptionsExt;
    // Chromium's ProcessSingleton keeps this file open without write sharing.
    // Job accounting may reach zero before Windows releases the native handle.
    let path = profile.join("lockfile");
    loop {
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(recovery("Cannot inspect Chromium profile lock")),
        };
        use std::os::windows::fs::MetadataExt;
        if !metadata.is_file() || metadata.file_attributes() & 0x400 != 0 {
            return Err(recovery("Unexpected Chromium profile lock file"));
        }
        match fs::OpenOptions::new()
            .write(true)
            .share_mode(0)
            .custom_flags(0x0020_0000)
            .open(&path)
        {
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error)
                if matches!(error.raw_os_error(), Some(32 | 33)) && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                return Err(recovery(
                    "Chromium profile lock is still held or unavailable",
                ));
            }
        }
    }
}

pub struct BrowserProcess {
    tree: OwnedProcessTree,
    leases: [StandaloneLease; 2],
    #[cfg(windows)]
    profile: PathBuf,
}

impl BrowserProcess {
    pub fn start(
        paths: SessionPaths,
        snapshot: &LaunchSnapshot,
        digest: &str,
        executable: &Path,
        configure: impl FnOnce(&SessionPaths) -> (Vec<OsString>, BTreeMap<String, OsString>),
    ) -> Result<Self, CoreError> {
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(CoreError::invalid("Invalid browser package digest"));
        }
        let paths = paths.prepare()?;
        let primary = StandaloneLease::acquire(
            &paths.root,
            &snapshot.profile_id,
            &snapshot.launch_id,
            &snapshot.idempotency_key,
        )?;
        let native = match StandaloneLease::acquire(
            &paths.profile,
            "profile",
            &snapshot.launch_id,
            &snapshot.idempotency_key,
        ) {
            Ok(lease) => lease,
            Err(error) => {
                primary.cancel()?;
                return Err(error);
            }
        };
        let bind = bind_profile(&paths.profile, digest);
        #[cfg(windows)]
        let bind = bind.and_then(|()| profile_available(&paths.profile, Instant::now()));
        if let Err(error) = bind.and_then(|()| record_intent(&paths.root, snapshot)) {
            native.cancel()?;
            primary.cancel()?;
            return Err(error);
        }
        let (arguments, environment) = configure(&paths);
        let mut leases = [primary, native];
        for lease in &mut leases {
            lease.transition(Lifecycle::Starting, None)?;
        }
        let spawned = OwnedProcessTree::spawn(
            executable,
            &arguments,
            executable
                .parent()
                .ok_or_else(|| CoreError::invalid("Missing executable directory"))?,
            &environment,
        );
        #[cfg(windows)]
        let spawned = spawned.map_err(|_| {
            CoreError::new(
                ErrorCode::LaunchFailure,
                "Could not start Windows browser in its own job",
            )
        });
        let tree = spawned?;
        let pid = tree.pid();
        let mut process = Self {
            tree,
            leases,
            #[cfg(windows)]
            profile: paths.profile,
        };
        for lease in &mut process.leases {
            lease.transition(Lifecycle::Running, Some(pid))?;
        }
        Ok(process)
    }

    pub fn root_exited(&self) -> Result<bool, CoreError> {
        #[cfg(target_os = "linux")]
        return self.tree.root_exited();
        #[cfg(windows)]
        self.tree
            .root_exited()
            .map_err(|_| recovery("Cannot observe the Windows browser process"))
    }

    pub fn stop(mut self, deadline: Instant) -> Result<(), CoreError> {
        let marked = self
            .leases
            .iter_mut()
            .try_for_each(|lease| lease.transition(Lifecycle::Stopping, None));
        let stopped = self
            .tree
            .terminate_and_wait(deadline.saturating_duration_since(Instant::now()));
        #[cfg(windows)]
        let stopped = stopped
            .map_err(|_| recovery("Windows browser process tree did not stop"))
            .and_then(|()| profile_available(&self.profile, deadline));
        marked.and(stopped)?;
        for lease in &mut self.leases {
            lease.transition(Lifecycle::Stopped, None)?;
        }
        Ok(())
    }
}
