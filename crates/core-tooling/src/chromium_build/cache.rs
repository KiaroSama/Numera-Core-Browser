//! Validate the original Python checkpoint format before restoring compiler outputs.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, BufReader, Read, Seek},
    path::{Path, PathBuf},
};
use tar::{Archive, Builder, Header};

pub type Inventory = BTreeMap<String, String>;

const MANIFEST: &str = "manifest.json";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
pub const PROBE_CONFIG: &str = "direct_mode = false\ncompiler_check = content\nmax_size = 24M\n";
pub const STAGED_CONFIG: &str = "direct_mode = false\ncompiler_check = content\nmax_size = 2550M\n";

#[derive(Clone, Copy)]
pub struct Limits {
    bytes: u64,
    entries: usize,
}

pub const PROBE_LIMITS: Limits = Limits {
    bytes: 32 * 1024 * 1024,
    entries: 512,
};
pub const SEGMENT_LIMITS: Limits = Limits {
    bytes: 2_650_000_000,
    entries: 100_000,
};
pub const OBJECT_DELTA_LIMITS: Limits = Limits {
    bytes: PROBE_LIMITS.bytes,
    entries: SEGMENT_LIMITS.entries,
};

#[derive(Clone, Copy)]
pub struct Delta<'a> {
    pub base_sha256: &'a str,
    pub base_entries: &'a Inventory,
}

pub struct Restore<'a> {
    pub context: &'a Value,
    pub expected_sha256: &'a str,
    pub base_sha256: Option<&'a str>,
    pub allow_linux_runner_image: bool,
    pub limits: Limits,
}

impl<'a> Restore<'a> {
    pub fn new(context: &'a Value, expected_sha256: &'a str, limits: Limits) -> Self {
        Self {
            context,
            expected_sha256,
            limits,
            base_sha256: None,
            allow_linux_runner_image: false,
        }
    }
}

#[derive(Debug)]
pub struct Restored {
    pub object_sha256: Option<String>,
    pub runner_images: Option<Value>,
    pub entries_added: usize,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    context: Value,
    object_sha256: Option<String>,
    files: Inventory,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_checkpoint_sha256: Option<String>,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

pub fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hash_reader(mut bytes: impl Read) -> io::Result<String> {
    let mut state = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = bytes.read(&mut buffer)?;
        if count == 0 {
            return Ok(hex(&state.finalize()));
        }
        state.update(&buffer[..count]);
    }
}

pub fn sha256(path: &Path) -> io::Result<String> {
    hash_reader(File::open(path)?)
}

fn valid_member(name: &str) -> bool {
    name.split('/').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
}

fn collect(
    root: &Path,
    current: &Path,
    files: &mut BTreeMap<String, PathBuf>,
    limit: usize,
) -> io::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            collect(root, &path, files, limit)?;
            continue;
        }
        if !kind.is_file() {
            return Err(invalid(
                "Unexpected link or special file in local compiler cache",
            ));
        }
        let name = entry.file_name();
        if matches!(name.to_str(), Some("CACHEDIR.TAG" | ".cleaned" | "stats")) {
            continue;
        }
        let relative = path.strip_prefix(root).map_err(io::Error::other)?;
        let mut parts = Vec::new();
        for component in relative.components() {
            parts.push(
                component
                    .as_os_str()
                    .to_str()
                    .ok_or_else(|| invalid("Non-UTF-8 compiler cache path"))?,
            );
        }
        let name = parts.join("/");
        if !valid_member(&name) || files.len() == limit {
            return Err(invalid(
                "Compiler cache paths or entry count are out of bounds",
            ));
        }
        files.insert(name, path);
    }
    Ok(())
}

pub fn pack_cache(
    root: &Path,
    archive: &Path,
    context: &Value,
    object_sha256: Option<String>,
    limits: Limits,
    required_entries: Option<&Inventory>,
    delta: Option<Delta<'_>>,
) -> io::Result<()> {
    let mut files = BTreeMap::new();
    collect(root, root, &mut files, limits.entries)?;
    let mut names = BTreeSet::new();
    if files
        .keys()
        .any(|name| !names.insert(name.to_ascii_lowercase()))
    {
        return Err(invalid("Conflicting compiler cache member names"));
    }
    if let Some(base) = delta {
        if !valid_sha256(base.base_sha256) || base.base_entries.is_empty() {
            return Err(invalid(
                "A compiler cache delta requires a verified base checkpoint",
            ));
        }
        for (name, digest) in base.base_entries {
            let file = files.remove(name).ok_or_else(|| {
                invalid("Previously restored cache entries were evicted or changed")
            })?;
            if sha256(&file)? != *digest {
                return Err(invalid(
                    "Previously restored cache entries were evicted or changed",
                ));
            }
        }
    }
    let mut size = 0_u64;
    let mut hashes = Inventory::new();
    for (name, path) in &files {
        size = size
            .checked_add(path.metadata()?.len())
            .ok_or_else(|| invalid("Compiler cache exceeds its transport limit"))?;
        if size >= limits.bytes.saturating_sub(1024 * 1024) {
            return Err(invalid("Compiler cache exceeds its transport limit"));
        }
        hashes.insert(name.clone(), sha256(path)?);
    }
    if hashes.is_empty() {
        return Err(invalid("Compiler cache is empty"));
    }
    if let Some(required) = required_entries {
        for (name, digest) in required {
            let actual = hashes
                .get(name)
                .or_else(|| delta.and_then(|base| base.base_entries.get(name)));
            if actual != Some(digest) {
                return Err(invalid(
                    "Previously restored cache entries were evicted or changed",
                ));
            }
        }
    }
    let manifest = Manifest {
        context: context.clone(),
        object_sha256,
        files: hashes,
        base_checkpoint_sha256: delta.map(|base| base.base_sha256.to_owned()),
    };
    let contents = serde_json::to_vec(&manifest)?;
    if contents.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(invalid("Compiler cache manifest is out of bounds"));
    }
    let parent = archive
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let staged = tempfile::NamedTempFile::new_in(parent)?;
    let mut builder = Builder::new(staged);
    builder.follow_symlinks(false);
    let mut header = Header::new_ustar();
    header.set_size(contents.len() as u64);
    header.set_mode(0o644);
    builder.append_data(&mut header, MANIFEST, contents.as_slice())?;
    for (name, file) in files {
        builder.append_path_with_name(file, name)?;
    }
    let staged = builder.into_inner()?;
    if staged.as_file().metadata()?.len() > limits.bytes {
        return Err(invalid(
            "Compiler cache archive exceeds the configured transport limit",
        ));
    }
    staged
        .persist_noclobber(archive)
        .map_err(|error| error.error)?;
    Ok(())
}

fn validate_context(restore: &Restore<'_>, producer: &Value) -> io::Result<Option<Value>> {
    let context = restore
        .context
        .as_object()
        .ok_or_else(|| invalid("Invalid compiler cache context"))?;
    let producer = producer
        .as_object()
        .ok_or_else(|| invalid("Invalid compiler cache context"))?;
    let keys: BTreeSet<_> = context.keys().chain(producer.keys()).collect();
    let changes: Vec<_> = keys
        .into_iter()
        .filter(|key| context.get(*key) != producer.get(*key))
        .map(String::as_str)
        .collect();
    let images = serde_json::json!({"seed": producer.get("runner_image"), "resume": context.get("runner_image")});
    let compatible_image = restore.allow_linux_runner_image
        && changes == ["runner_image"]
        && context.get("host").and_then(Value::as_str) == Some("Linux")
        && context.get("cache_tool").and_then(Value::as_str) == Some("ccache")
        && [PROBE_CONFIG, STAGED_CONFIG].into_iter().any(|config| {
            context.get("cache_config_sha256").and_then(Value::as_str)
                == Some(&hex(&Sha256::digest(config)))
        })
        && ["seed", "resume"]
            .into_iter()
            .all(|key| images[key].as_str().is_some_and(|image| !image.is_empty()));
    if !changes.is_empty() && !compatible_image {
        return Err(invalid(format!(
            "Checkpoint build context changed: {}",
            changes.join(", ")
        )));
    }
    Ok(restore.allow_linux_runner_image.then_some(images))
}

fn read_archive(path: &Path, file: &mut File, limits: Limits) -> io::Result<(Manifest, Inventory)> {
    let mut archive = Archive::new(BufReader::new(file));
    let mut contents = None;
    let mut staged = Inventory::new();
    let mut names = BTreeSet::new();
    let mut bytes = 0_u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let member = entry.path_bytes();
        let name = std::str::from_utf8(&member)
            .map_err(io::Error::other)?
            .to_owned();
        bytes = bytes
            .checked_add(entry.size())
            .ok_or_else(|| invalid("Compiler cache archive size overflow"))?;
        if names.len() > limits.entries
            || bytes > limits.bytes
            || !entry.header().entry_type().is_file()
            || (name != MANIFEST && !valid_member(&name))
            || !names.insert(name.to_ascii_lowercase())
        {
            return Err(invalid("Unexpected compiler-cache archive members"));
        }
        if name == MANIFEST {
            if entry.size() > MAX_MANIFEST_BYTES {
                return Err(invalid("Compiler cache manifest is out of bounds"));
            }
            let mut content = Vec::new();
            entry.read_to_end(&mut content)?;
            contents = Some(serde_json::from_slice::<Manifest>(&content)?);
        } else {
            staged.insert(name, hash_reader(entry)?);
        }
    }
    let manifest = contents.ok_or_else(|| invalid("Missing compiler cache manifest"))?;
    if staged.is_empty() || manifest.files.keys().ne(staged.keys()) {
        return Err(invalid("Checkpoint inventory is incomplete"));
    }
    if manifest.files != staged {
        return Err(invalid("Checkpoint cache member digest mismatch"));
    }
    if path.symlink_metadata()?.file_type().is_symlink() {
        return Err(invalid("Compiler cache archive must be a local file"));
    }
    Ok((manifest, staged))
}

fn check_directory(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(invalid(
            "Compiler cache directory is not a regular directory",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub fn restore_cache(
    root: &Path,
    archive: &Path,
    restore: &Restore<'_>,
    entries: &mut Inventory,
) -> io::Result<Restored> {
    let metadata = archive.symlink_metadata()?;
    if !metadata.file_type().is_file()
        || metadata.len() > restore.limits.bytes
        || !valid_sha256(restore.expected_sha256)
    {
        return Err(invalid(
            "Checkpoint does not match this attempt's seed digest",
        ));
    }
    let mut file = File::open(archive)?;
    if sha256_reader(&mut file)? != restore.expected_sha256 {
        return Err(invalid(
            "Checkpoint does not match this attempt's seed digest",
        ));
    }
    file.rewind()?;
    let (manifest, staged) = read_archive(archive, &mut file, restore.limits)?;
    if manifest.base_checkpoint_sha256.as_deref() != restore.base_sha256
        || restore
            .base_sha256
            .is_some_and(|digest| entries.is_empty() || !valid_sha256(digest))
    {
        return Err(invalid(
            "Compiler cache delta does not match the restored base checkpoint",
        ));
    }
    let runner_images = validate_context(restore, &manifest.context)?;
    let existing: BTreeMap<_, _> = entries
        .iter()
        .map(|(name, digest)| (name.to_ascii_lowercase(), (name, digest)))
        .collect();
    let pending: BTreeSet<_> = staged
        .keys()
        .map(|name| name.to_ascii_lowercase())
        .collect();
    check_directory(root)?;
    for (name, digest) in &staged {
        let target = root.join(name);
        let mut directory = root.to_path_buf();
        let mut prefix = String::new();
        let parts: Vec<_> = name.split('/').collect();
        for part in &parts[..parts.len() - 1] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            if pending.contains(&prefix.to_ascii_lowercase()) {
                return Err(invalid("A compiler cache member overlaps a directory"));
            }
            directory.push(part);
            check_directory(&directory)?;
        }
        if let Some((previous, previous_digest)) = existing.get(&name.to_ascii_lowercase()) {
            if restore.base_sha256.is_none()
                || *previous != name
                || *previous_digest != digest
                || !target.symlink_metadata()?.file_type().is_file()
                || sha256(&target)? != *digest
            {
                return Err(invalid(
                    "Conflicting compiler cache entries from independent jobs",
                ));
            }
        } else if target.symlink_metadata().is_ok() {
            return Err(invalid("Unverified compiler cache member already exists"));
        }
    }
    file.rewind()?;
    let mut archive = Archive::new(BufReader::new(file));
    let mut added = 0;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let member = entry.path_bytes();
        let name = std::str::from_utf8(&member).map_err(io::Error::other)?;
        if name == MANIFEST || existing.contains_key(&name.to_ascii_lowercase()) {
            continue;
        }
        let expected = staged
            .get(name)
            .ok_or_else(|| invalid("Compiler cache changed during restore"))?;
        let target = root.join(name);
        let parent = target
            .parent()
            .ok_or_else(|| invalid("Missing compiler cache destination parent"))?;
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        io::copy(&mut entry, temporary.as_file_mut())?;
        if sha256(temporary.path())? != *expected {
            return Err(invalid("Compiler cache changed during restore"));
        }
        temporary
            .persist_noclobber(target)
            .map_err(|error| error.error)?;
        added += 1;
    }
    if added
        + staged
            .keys()
            .filter(|name| existing.contains_key(&name.to_ascii_lowercase()))
            .count()
        != staged.len()
    {
        return Err(invalid("Compiler cache changed during restore"));
    }
    entries.extend(staged);
    Ok(Restored {
        object_sha256: manifest.object_sha256,
        runner_images,
        entries_added: added,
    })
}

fn sha256_reader(file: &mut File) -> io::Result<String> {
    hash_reader(BufReader::new(file))
}
