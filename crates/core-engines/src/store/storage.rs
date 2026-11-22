use core_contracts::{CoreError, ErrorCode};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

pub(super) fn recovery() -> CoreError {
    CoreError::new(
        ErrorCode::RecoveryRequired,
        "The engine store could not be read or is inconsistent",
    )
}

pub(super) fn storage_error(error: io::Error) -> CoreError {
    if error.kind() == io::ErrorKind::StorageFull {
        CoreError::new(
            ErrorCode::InsufficientResources,
            "The engine store has insufficient storage space",
        )
    } else {
        recovery()
    }
}

pub(super) fn missing() -> CoreError {
    CoreError::new(
        ErrorCode::MissingPackage,
        "No installed Chromium package matches the selected build",
    )
}

pub(super) fn signature() -> CoreError {
    CoreError::new(
        ErrorCode::UpdateSignatureFailure,
        "Offline engine metadata or payload failed authentication, freshness, or rollback checks",
    )
}

pub(super) fn absolute_directory(path: &Path) -> io::Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let path = std::path::absolute(path)?;
    if path
        .components()
        .any(|part| part == std::path::Component::ParentDir)
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    #[cfg(windows)]
    if !matches!(
        path.components().next(),
        Some(std::path::Component::Prefix(prefix))
            if matches!(prefix.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_))
    ) {
        return Err(io::ErrorKind::InvalidInput.into());
    }

    // Validate existing ancestors from the root down. An absent leaf may be
    // selected for a future install.
    for ancestor in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        match directory(ancestor) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(error),
        }
    }
    Ok(path)
}

pub(super) fn directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || link(&metadata) {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(())
}

pub(super) fn private_directory(path: &Path) -> io::Result<()> {
    directory(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::symlink_metadata(path)?.permissions().mode() & 0o077 != 0 {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
    }
    Ok(())
}

pub(super) fn create_private_directory(path: &Path) -> io::Result<()> {
    let mut new_ancestors = Vec::new();
    for ancestor in path.ancestors() {
        new_ancestors.push(ancestor);
        match directory(ancestor) {
            Ok(()) => break,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let mut options = fs::DirBuilder::new();
    options.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        options.mode(0o700);
    }
    options.create(path)?;
    private_directory(path)?;
    for ancestor in new_ancestors {
        sync_dir(ancestor)?;
    }
    Ok(())
}

pub(super) fn link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & REPARSE_POINT != 0 {
            return true;
        }
    }
    metadata.is_symlink()
}

pub(super) fn regular(path: &Path, create: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(create).create(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Open and fstat the same descriptor. O_NONBLOCK avoids waiting on
        // an attacker-supplied FIFO before rejecting its type.
        options
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(OPEN_REPARSE_POINT);
    }

    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || link(&metadata) {
        return Err(io::ErrorKind::InvalidData.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(io::ErrorKind::InvalidData.into());
        }
    }
    Ok(file)
}

pub(super) fn bounded_read(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let file = regular(path, false)?;
    if file.metadata()?.len() > limit {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(bytes)
}

pub(super) fn parse<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CoreError> {
    serde_json::from_slice(bytes).map_err(|_| recovery())
}

pub(super) fn bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, CoreError> {
    serde_json::to_vec(value).map_err(|_| recovery())
}

pub(super) fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn write_state(path: &Path, value: &[u8]) -> Result<(), CoreError> {
    let parent = path.parent().ok_or_else(recovery)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(storage_error)?;
    temporary.write_all(value).map_err(storage_error)?;
    temporary.as_file().sync_all().map_err(storage_error)?;
    let staged = temporary.into_temp_path();
    publish(&staged, path, true).map_err(storage_error)?;
    sync_dir(parent).map_err(storage_error)
}

#[cfg(unix)]
pub(super) fn publish(from: &Path, to: &Path, _replace: bool) -> io::Result<()> {
    // The administrative lock serializes writers and final packages are
    // checked for existence before publication.
    fs::rename(from, to)
}

#[cfg(windows)]
#[allow(unsafe_code)]
pub(super) fn publish(from: &Path, to: &Path, replace: bool) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_NORMAL, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        SetFileAttributesW,
    };

    fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
        let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
        let name = path.file_name().ok_or(io::ErrorKind::InvalidInput)?;
        let absolute = fs::canonicalize(parent)?.join(name);
        Ok(absolute.as_os_str().encode_wide().chain([0]).collect())
    }

    let source = wide_path(from)?;
    let destination = wide_path(to)?;
    let flags = if replace {
        MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH
    } else {
        MOVEFILE_WRITE_THROUGH
    };
    // SAFETY: Both UTF-16 name buffers end in NUL and remain alive for both
    // calls. Publication is on the same volume and never enables copy fallback.
    unsafe {
        // The temporary state file must retain ordinary file attributes once
        // published. Directory publication does not alter directory attributes.
        if (replace && SetFileAttributesW(source.as_ptr(), FILE_ATTRIBUTE_NORMAL) == 0)
            || MoveFileExW(source.as_ptr(), destination.as_ptr(), flags) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

pub(super) fn sync_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(windows)]
    let _ = path;
    Ok(())
}
