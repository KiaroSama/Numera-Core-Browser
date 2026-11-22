//! Read-only inspection of Numera's portable runtime archive format.
use crate::{digest_string, open_regular_file, safe_relative};
use core_contracts::CoreError;
use flate2::bufread::GzDecoder;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    io::{self, BufRead, BufReader, Read},
    path::Path,
};

pub(crate) const MAX_ARCHIVE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_TAR_BYTES: u64 = 24 * 1024 * 1024 * 1024;
pub(crate) const MAX_MEMBERS: usize = 150_000;
const MAX_PATH_BYTES: usize = 4096;
const MAX_ALL_PATH_BYTES: usize = 16 * 1024 * 1024;

#[derive(Default)]
struct Paths {
    files: BTreeSet<String>,
    directories: BTreeMap<String, String>,
    file_path_bytes: usize,
    directory_path_bytes: usize,
}

impl Paths {
    fn add(&mut self, relative: &str) -> Result<(), CoreError> {
        self.file_path_bytes += relative.len();
        if relative.len() > MAX_PATH_BYTES
            || self.file_path_bytes > MAX_ALL_PATH_BYTES
            || !safe_relative(relative)
            || self.files.len() >= MAX_MEMBERS
        {
            return Err(invalid_archive());
        }
        let key = relative.to_uppercase();
        if self.directories.contains_key(&key) || self.files.contains(&key) {
            return Err(invalid_archive());
        }
        let (mut folded, mut path) = (key.as_str(), relative);
        while let Some(((prefix, _), (spelling, _))) =
            folded.rsplit_once('/').zip(path.rsplit_once('/'))
        {
            if self.files.contains(prefix) {
                return Err(invalid_archive());
            }
            let count = self.directories.len();
            match self.directories.entry(prefix.to_owned()) {
                Entry::Occupied(known) if known.get() == spelling => {}
                Entry::Vacant(new) if count < MAX_MEMBERS => {
                    self.directory_path_bytes += prefix.len() + spelling.len();
                    if self.directory_path_bytes > MAX_ALL_PATH_BYTES {
                        return Err(invalid_archive());
                    }
                    new.insert(spelling.to_owned());
                }
                _ => return Err(invalid_archive()),
            }
            (folded, path) = (prefix, spelling);
        }
        self.files.insert(key);
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct RuntimeArchiveReport {
    pub archive_bytes: u64,
    pub sha256: String,
    pub files: usize,
    pub file_bytes: u64,
}

struct HashedInput<R> {
    input: R,
    digest: Sha256,
    bytes: u64,
}

impl<R: Read> Read for HashedInput<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.input.read(buffer)?;
        self.digest.update(&buffer[..count]);
        self.bytes += count as u64;
        Ok(count)
    }
}

fn invalid_archive() -> CoreError {
    CoreError::invalid("Invalid or unsupported portable runtime archive")
}

fn name_extension<R: Read>(entry: &mut tar::Entry<'_, R>) -> Result<String, CoreError> {
    if entry.size() > (MAX_PATH_BYTES + 64) as u64
        || (entry.header().as_gnu().is_none() && entry.header().as_ustar().is_none())
    {
        return Err(invalid_archive());
    }
    let mut bytes = Vec::new();
    entry
        .read_to_end(&mut bytes)
        .map_err(|_| invalid_archive())?;
    if entry.header().entry_type().is_gnu_longname() {
        if bytes.pop() != Some(0) {
            return Err(invalid_archive());
        }
        return String::from_utf8(bytes).map_err(|_| invalid_archive());
    }
    // Numera emits only a single PAX path record, with its final LF included in the length.
    let record = std::str::from_utf8(&bytes).map_err(|_| invalid_archive())?;
    let (length, contents) = record.split_once(' ').ok_or_else(invalid_archive)?;
    if !length.bytes().all(|byte| byte.is_ascii_digit())
        || length.parse::<usize>() != Ok(bytes.len())
    {
        return Err(invalid_archive());
    }
    contents
        .strip_prefix("path=")
        .and_then(|path| path.strip_suffix('\n'))
        .map(str::to_owned)
        .ok_or_else(invalid_archive)
}

fn inspect_archive<R: Read>(
    input: R,
    expected: &str,
    tar_limit: u64,
) -> Result<RuntimeArchiveReport, CoreError> {
    walk_archive(input, expected, tar_limit, |_, _, member| {
        io::copy(member, &mut io::sink()).map_err(|_| invalid_archive())
    })
}

fn walk_archive<R: Read>(
    input: R,
    expected: &str,
    tar_limit: u64,
    mut process: impl FnMut(&str, u32, &mut dyn Read) -> Result<u64, CoreError>,
) -> Result<RuntimeArchiveReport, CoreError> {
    let source = HashedInput {
        input: input.take(MAX_ARCHIVE_BYTES + 1),
        digest: Sha256::new(),
        bytes: 0,
    };
    let decoder = GzDecoder::new(BufReader::new(source));
    let mut archive = tar::Archive::new(decoder.take(tar_limit + 1));
    let mut paths = Paths::default();
    let mut declared_path = None;
    let mut file_bytes = 0_u64;
    for entry in archive.entries().map_err(|_| invalid_archive())?.raw(true) {
        let mut entry = entry.map_err(|_| invalid_archive())?;
        let kind = entry.header().entry_type();
        if kind.is_gnu_longname() || kind.is_pax_local_extensions() {
            if declared_path.is_some() {
                return Err(invalid_archive());
            }
            declared_path = Some(name_extension(&mut entry)?);
            continue;
        }
        let mode = entry.header().mode().map_err(|_| invalid_archive())?;
        if !kind.is_file() || mode & 0o7000 != 0 {
            return Err(invalid_archive());
        }
        let path = match declared_path.take() {
            Some(path) => path,
            None => {
                String::from_utf8(entry.path_bytes().into_owned()).map_err(|_| invalid_archive())?
            }
        };
        let relative = path.strip_prefix("runtime/").ok_or_else(invalid_archive)?;
        paths.add(relative)?;
        let count = process(relative, 0o444 | (mode & 0o111), &mut entry)?;
        if count != entry.size() {
            return Err(invalid_archive());
        }
        file_bytes += count;
    }
    if paths.files.is_empty() || declared_path.is_some() {
        return Err(invalid_archive());
    }

    // The parser stops after the first end marker. Read the remaining padding and
    // gzip checksum, and reject a second archive or bytes after the gzip member.
    let mut expanded = archive.into_inner();
    let mut tail = Vec::new();
    (&mut expanded)
        .take(10_241)
        .read_to_end(&mut tail)
        .map_err(|_| invalid_archive())?;
    if !(512..=10_240).contains(&tail.len())
        || !tail.len().is_multiple_of(512)
        || tail.iter().any(|byte| *byte != 0)
        || expanded.limit() == 0
    {
        return Err(invalid_archive());
    }
    let mut compressed = expanded.into_inner().into_inner();
    if !compressed
        .fill_buf()
        .map_err(|_| invalid_archive())?
        .is_empty()
    {
        return Err(invalid_archive());
    }
    let source = compressed.into_inner();
    if source.bytes > MAX_ARCHIVE_BYTES {
        return Err(invalid_archive());
    }
    let sha256: String = source
        .digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if !sha256.eq_ignore_ascii_case(expected) {
        return Err(CoreError::invalid("Runtime archive SHA-256 does not match"));
    }
    Ok(RuntimeArchiveReport {
        archive_bytes: source.bytes,
        sha256,
        files: paths.files.len(),
        file_bytes,
    })
}

#[cfg(feature = "package-store")]
pub(crate) fn read_runtime_members<R: Read>(
    input: R,
    expected: &str,
    process: impl FnMut(&str, u32, &mut dyn Read) -> Result<u64, CoreError>,
) -> Result<RuntimeArchiveReport, CoreError> {
    walk_archive(input, expected, MAX_TAR_BYTES, process)
}

pub fn inspect_runtime_archive(
    path: &Path,
    expected_sha256: &str,
) -> Result<RuntimeArchiveReport, CoreError> {
    if !digest_string(expected_sha256) {
        return Err(CoreError::invalid(
            "Expected a SHA-256 digest for the runtime archive",
        ));
    }
    let file = open_regular_file(path)?;
    if file.metadata().map_err(|_| invalid_archive())?.len() > MAX_ARCHIVE_BYTES {
        return Err(invalid_archive());
    }
    inspect_archive(file, expected_sha256, MAX_TAR_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use std::io::Cursor;

    fn tar_fixture(files: &[(&str, &[u8])]) -> Vec<u8> {
        let compressed = GzEncoder::new(Vec::new(), Compression::fast());
        let mut archive = tar::Builder::new(compressed);
        for (path, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            archive.append_data(&mut header, path, *data).unwrap();
        }
        archive.into_inner().unwrap().finish().unwrap()
    }

    fn check(bytes: &[u8], expanded_limit: u64) -> Result<RuntimeArchiveReport, CoreError> {
        let expected: String = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        inspect_archive(Cursor::new(bytes), &expected, expanded_limit)
    }

    #[test]
    fn validates_archive_hash_and_reports_exact_expanded_files() {
        let bytes = tar_fixture(&[
            ("runtime/chrome", b"browser"),
            ("runtime/locales/فارسی.pak", b"asset"),
        ]);
        let report = check(&bytes, 8192).unwrap();
        assert_eq!(report.files, 2);
        assert_eq!(report.file_bytes, 12);
        assert_eq!(report.archive_bytes, bytes.len() as u64);
        assert!(inspect_archive(Cursor::new(bytes), &"0".repeat(64), 8192).is_err());
    }

    #[test]
    fn rejects_windows_aliases_duplicates_and_file_directory_conflicts() {
        for invalid in [
            vec![("runtime/CON.txt", b"x".as_slice())],
            vec![
                ("runtime/chrome", b"x".as_slice()),
                ("runtime/CHROME", b"y"),
            ],
            vec![("runtime/Σ.pak", b"x".as_slice()), ("runtime/ς.pak", b"y")],
            vec![
                ("runtime/Locales/a.pak", b"x".as_slice()),
                ("runtime/locales/b.pak", b"y"),
            ],
            vec![
                ("runtime/data", b"x".as_slice()),
                ("runtime/data/inner", b"y"),
            ],
        ] {
            assert!(check(&tar_fixture(&invalid), 8192).is_err());
        }
    }

    #[test]
    fn rejects_oversized_expansion_and_trailing_compressed_data() {
        let mut bytes = tar_fixture(&[("runtime/chrome", &vec![0; 2048])]);
        assert!(check(&bytes, 1024).is_err());
        bytes.extend_from_slice(b"trailing bytes");
        assert!(check(&bytes, 8192).is_err());
    }

    #[test]
    fn reads_bounded_pax_paths_and_rejects_links() {
        let compressor = GzEncoder::new(Vec::new(), Compression::fast());
        let mut tar = tar::Builder::new(compressor);
        let expanded = "runtime/locales/داده‌های مرورگر.pak".as_bytes();
        tar.append_pax_extensions([("path", expanded)]).unwrap();
        let mut header = tar::Header::new_ustar();
        header.set_size(1);
        header.set_mode(0o644);
        tar.append_data(&mut header, "runtime/pax-filename", &b"x"[..])
            .unwrap();
        let bytes = tar.into_inner().unwrap().finish().unwrap();
        assert_eq!(check(&bytes, 8192).unwrap().files, 1);

        let compressor = GzEncoder::new(Vec::new(), Compression::fast());
        let mut tar = tar::Builder::new(compressor);
        let mut header = tar::Header::new_ustar();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_mode(0o777);
        tar.append_link(&mut header, "runtime/chrome", "chromium")
            .unwrap();
        let bytes = tar.into_inner().unwrap().finish().unwrap();
        assert!(check(&bytes, 8192).is_err());
    }

    #[test]
    fn rejects_pax_path_without_its_record_terminator() {
        let compressor = GzEncoder::new(Vec::new(), Compression::fast());
        let mut tar = tar::Builder::new(compressor);
        let extended = b"23 path=runtime/chrome";
        let mut pax = tar::Header::new_ustar();
        pax.set_entry_type(tar::EntryType::XHeader);
        pax.set_size(extended.len() as u64);
        pax.set_mode(0o644);
        tar.append_data(&mut pax, "PaxHeader", &extended[..])
            .unwrap();
        let mut header = tar::Header::new_ustar();
        header.set_size(1);
        header.set_mode(0o755);
        tar.append_data(&mut header, "runtime/chrome", &b"x"[..])
            .unwrap();
        let bytes = tar.into_inner().unwrap().finish().unwrap();
        assert!(check(&bytes, 8192).is_err());
    }
}
