use super::storage::{
    bounded_read, bytes, directory, hex, link, recovery, regular, sha256, signature, storage_error,
    sync_dir,
};
use crate::runtime_archive::{MAX_ARCHIVE_BYTES, MAX_MEMBERS, read_runtime_members};
use crate::{EngineManifest, digest_string, inspect_manifest, safe_relative};
use core_contracts::CoreError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, Write},
    path::Path,
};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Member {
    bytes: u64,
    sha256: String,
    mode: u32,
}

pub(super) type Members = BTreeMap<String, Member>;

struct DigestWriter<'a> {
    destination: Option<&'a mut File>,
    digest: Sha256,
}

impl Write for DigestWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(file) = self.destination.as_mut() {
            file.write_all(bytes)?;
        }
        self.digest.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn read_member(
    source: &mut dyn Read,
    destination: Option<&mut File>,
    mode: u32,
) -> io::Result<Member> {
    let mut writer = DigestWriter {
        destination,
        digest: Sha256::new(),
    };
    let bytes = io::copy(source, &mut writer)?;
    Ok(Member {
        bytes,
        sha256: hex(&writer.digest.finalize()),
        mode,
    })
}

pub(super) fn verify_inventory(
    members: &Members,
    manifest: &EngineManifest,
    expected: &str,
) -> Result<(), CoreError> {
    if members.is_empty()
        || members.len() > MAX_MEMBERS
        || !members.iter().all(|(path, member)| {
            safe_relative(path) && digest_string(&member.sha256) && member.mode & !0o111 == 0o444
        })
        || !members.get(&manifest.executable).is_some_and(|member| {
            member
                .sha256
                .eq_ignore_ascii_case(&manifest.executable_sha256)
                && (manifest.target_triple != "x86_64-unknown-linux-gnu"
                    || member.mode & 0o100 != 0)
        })
        || !manifest
            .licenses
            .iter()
            .all(|path| members.contains_key(path))
        || sha256(&bytes(members)?) != expected
    {
        return Err(signature());
    }
    Ok(())
}

pub(super) fn copy_archive(
    source: &Path,
    destination: &Path,
    digest: &str,
    length: u64,
) -> Result<File, CoreError> {
    let input = regular(source, false).map_err(|_| signature())?;
    if length == 0
        || length > MAX_ARCHIVE_BYTES
        || input.metadata().map_err(|_| signature())?.len() != length
    {
        return Err(signature());
    }
    let mut output = tempfile::tempfile_in(destination).map_err(storage_error)?;
    let copied =
        read_member(&mut input.take(length + 1), Some(&mut output), 0).map_err(storage_error)?;
    if copied.bytes != length || copied.sha256 != digest {
        return Err(signature());
    }
    output.rewind().map_err(storage_error)?;
    Ok(output)
}

pub(super) fn unpack(archive: File, digest: &str, runtime: &Path) -> Result<Members, CoreError> {
    fs::create_dir(runtime).map_err(storage_error)?;
    let mut members = Members::new();
    let mut dirs = BTreeSet::new();
    let report = read_runtime_members(archive, digest, |relative, mode, source| {
        let path = runtime.join(relative);
        let parent = path.parent().ok_or_else(recovery)?;
        fs::create_dir_all(parent).map_err(storage_error)?;
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(storage_error)?;
        let member = read_member(source, Some(&mut output), mode).map_err(storage_error)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            output
                .set_permissions(fs::Permissions::from_mode(mode))
                .map_err(storage_error)?;
        }
        output.sync_all().map_err(storage_error)?;
        members.insert(relative.to_owned(), member.clone());
        dirs.extend(
            parent
                .ancestors()
                .take_while(|path| path.starts_with(runtime))
                .map(Path::to_owned),
        );
        Ok(member.bytes)
    })
    .map_err(|error| {
        if error.code == core_contracts::ErrorCode::InvalidConfiguration {
            signature()
        } else {
            error
        }
    })?;
    if report.files != members.len() {
        return Err(signature());
    }
    for path in dirs.iter().rev() {
        sync_dir(path).map_err(storage_error)?;
    }
    Ok(members)
}

pub(super) fn verify_installed(runtime: &Path, members: &Members) -> Result<(), CoreError> {
    directory(runtime).map_err(storage_error)?;
    let mut files_seen = 0;
    let mut pending = vec![runtime.to_owned()];
    let dirs: BTreeSet<_> = members
        .keys()
        .flat_map(|path| {
            Path::new(path)
                .ancestors()
                .skip(1)
                .filter(|part| !part.as_os_str().is_empty())
        })
        .collect();

    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).map_err(storage_error)? {
            let entry = entry.map_err(storage_error)?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(storage_error)?;
            if link(&metadata) {
                return Err(recovery());
            }
            let path = entry.path();
            let relative = path.strip_prefix(runtime).map_err(|_| recovery())?;
            if metadata.is_dir() {
                if !dirs.contains(relative) {
                    return Err(recovery());
                }
                pending.push(path);
                continue;
            }
            let name = relative.to_str().ok_or_else(recovery)?.replace('\\', "/");
            let member = members.get(&name).ok_or_else(recovery)?;
            let input = regular(&path, false).map_err(storage_error)?;
            let actual = input.metadata().map_err(storage_error)?;
            if actual.len() != member.bytes {
                return Err(recovery());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if actual.permissions().mode() & 0o7777 != member.mode {
                    return Err(recovery());
                }
            }
            let found = read_member(&mut input.take(member.bytes + 1), None, member.mode)
                .map_err(storage_error)?;
            if found != *member {
                return Err(recovery());
            }
            files_seen += 1;
        }
    }
    if files_seen != members.len() {
        return Err(recovery());
    }
    Ok(())
}

/// Derive the signed per-file inventory digest from the existing portable archive format.
pub fn runtime_inventory_sha256(path: &Path, archive_sha256: &str) -> Result<String, CoreError> {
    let (_, members) = archive_inventory(path, archive_sha256)?;
    Ok(sha256(&bytes(&members)?))
}

#[derive(Debug, Serialize)]
pub struct RuntimeTargetDescriptor {
    pub length: u64,
    pub hashes: RuntimeTargetHashes,
    pub custom: RuntimeTargetCustom,
}

#[derive(Debug, Serialize)]
pub struct RuntimeTargetHashes {
    pub sha256: String,
}

#[derive(Debug, Serialize)]
pub struct RuntimeTargetCustom {
    pub numera_engine_manifest: EngineManifest,
    pub numera_runtime_inventory_sha256: String,
}

/// Prepare a TUF target descriptor for an unsigned portable archive.
pub fn describe_runtime_archive(
    path: &Path,
    archive_sha256: &str,
    manifest: &EngineManifest,
) -> Result<RuntimeTargetDescriptor, CoreError> {
    let manifest = inspect_manifest(&bytes(manifest)?)?;
    super::trust::validate_manifest(&manifest)?;
    let (archive, members) = archive_inventory(path, archive_sha256)?;
    let inventory = sha256(&bytes(&members)?);
    verify_inventory(&members, &manifest, &inventory)?;
    Ok(RuntimeTargetDescriptor {
        length: archive.archive_bytes,
        hashes: RuntimeTargetHashes {
            sha256: archive.sha256,
        },
        custom: RuntimeTargetCustom {
            numera_engine_manifest: manifest,
            numera_runtime_inventory_sha256: inventory,
        },
    })
}

fn archive_inventory(
    path: &Path,
    archive_sha256: &str,
) -> Result<(crate::RuntimeArchiveReport, Members), CoreError> {
    if !digest_string(archive_sha256) {
        return Err(CoreError::invalid(
            "Expected a SHA-256 digest for the runtime archive",
        ));
    }
    let file = regular(path, false).map_err(storage_error)?;
    let mut members = Members::new();
    let report = read_runtime_members(file, archive_sha256, |relative, mode, input| {
        let member = read_member(input, None, mode).map_err(storage_error)?;
        let count = member.bytes;
        members.insert(relative.to_owned(), member);
        Ok(count)
    })?;
    Ok((report, members))
}

pub(super) fn receipt_bytes(path: &Path) -> Result<Vec<u8>, CoreError> {
    bounded_read(path, 48 * 1024 * 1024).map_err(storage_error)
}
