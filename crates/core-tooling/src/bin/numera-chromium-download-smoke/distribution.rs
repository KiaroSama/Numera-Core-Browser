use core_tooling::{
    chromium_build::{
        cache::valid_sha256,
        metadata::{Host, REPOSITORY},
        process,
    },
    fixtures::Result,
    sha256_file,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, Metadata},
    io::{self, BufReader, Read, Seek},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use zip::ZipArchive;

const SIGNER: &str =
    "KiaroSama/Numera-Core-Browser/.github/workflows/chromium-development-provenance.yml";
pub(super) const MANIFEST: &str = "development-runtime.json";
pub(super) const RUNTIME: &str = "numera-portable-runtime.tar.gz";
pub(super) const SOURCE: &str = "numera-source.zip";
pub(super) const INVENTORY: &str = "runtime-inventory.json";
pub(super) const PROOF: &str = "verified-upstream-source.json";
const SUBJECTS: [&str; 3] = [MANIFEST, RUNTIME, SOURCE];
const MAX_METADATA: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(super) struct Selection<'a> {
    pub(super) host: Host,
    pub(super) revision: &'a str,
    pub(super) run: u64,
}

pub(super) fn failure(message: &str) -> Box<dyn std::error::Error> {
    io::Error::other(message.to_owned()).into()
}

pub(super) fn valid_revision(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn target(host: Host) -> &'static str {
    match host {
        Host::Linux => "x86_64-unknown-linux-gnu",
        Host::Windows => "x86_64-pc-windows-msvc",
    }
}

fn checked_metadata(path: &Path) -> Result<Metadata> {
    let metadata = path.symlink_metadata()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(failure("Unexpected link in the retained artifact"));
        }
    }
    if metadata.file_type().is_symlink() {
        return Err(failure("Unexpected link in the retained artifact"));
    }
    Ok(metadata)
}

fn regular_file(path: &Path) -> Result<File> {
    if !checked_metadata(path)?.is_file() {
        return Err(failure("Expected a regular retained artifact file"));
    }
    Ok(File::open(path)?)
}

pub(super) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    regular_file(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(failure("Retained artifact metadata exceeds its size limit"));
    }
    Ok(bytes)
}

pub(super) fn valid_relative(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['\\', ':'])
        && !name.chars().any(char::is_control)
        && name
            .split('/')
            .all(|part| !part.is_empty() && !matches!(part, "." | ".."))
}

fn identity(root: &Path, selected: Selection<'_>) -> Result<Value> {
    let bytes = read_bounded(&root.join(MANIFEST), (core_contracts::MAX_FRAME - 1) as u64)?;
    let manifest = core_contracts::strict_json(&bytes)?;
    if !valid_revision(selected.revision)
        || selected.run == 0
        || manifest["target"] != target(selected.host)
        || manifest["numera_commit"] != selected.revision
        || manifest["workflow_run"] != selected.run
        || manifest["trusted_release"] != false
        || manifest["build_mode"] != "development"
    {
        return Err(failure(
            "The downloaded manifest does not describe the selected development run",
        ));
    }
    Ok(manifest)
}

fn package_paths(root: &Path) -> Result<BTreeSet<String>> {
    if !checked_metadata(root)?.is_dir() {
        return Err(failure("Expected a retained artifact directory"));
    }
    let mut files = BTreeSet::new();
    let mut directories = vec![root.to_owned()];
    while let Some(directory) = directories.pop() {
        for entry in directory.read_dir()? {
            let path = entry?.path();
            let kind = checked_metadata(&path)?;
            if kind.is_dir() {
                directories.push(path);
            } else if kind.is_file() {
                let relative = path
                    .strip_prefix(root)?
                    .iter()
                    .map(|part| {
                        part.to_str()
                            .ok_or_else(|| failure("Retained artifact names must be UTF-8"))
                    })
                    .collect::<Result<Vec<_>>>()?
                    .join("/");
                if !valid_relative(&relative) || !files.insert(relative) {
                    return Err(failure("Invalid retained artifact path"));
                }
            } else {
                return Err(failure("Unexpected file type in the retained artifact"));
            }
        }
    }
    Ok(files)
}

fn verify_contents(root: &Path, manifest: &Value) -> Result<()> {
    let attachments = manifest["attachments_sha256"]
        .as_object()
        .ok_or_else(|| failure("Missing authenticated attachment inventory"))?;
    if attachments.get(INVENTORY) != Some(&manifest["runtime_inventory_sha256"])
        || !attachments.contains_key(PROOF)
        || SUBJECTS.iter().any(|name| attachments.contains_key(*name))
        || manifest["runtime"]["file"] != RUNTIME
        || manifest["numera_source"]["file"] != SOURCE
    {
        return Err(failure(
            "The authenticated manifest contains inconsistent attachments",
        ));
    }
    let mut expected: BTreeMap<_, _> = attachments
        .iter()
        .map(|(name, digest)| (name.as_str(), digest))
        .collect();
    expected.insert(RUNTIME, &manifest["runtime"]["sha256"]);
    expected.insert(SOURCE, &manifest["numera_source"]["sha256"]);
    if !expected
        .iter()
        .all(|(name, digest)| valid_relative(name) && digest.as_str().is_some_and(valid_sha256))
    {
        return Err(failure(
            "The authenticated manifest contains invalid attachment metadata",
        ));
    }
    let actual = package_paths(root)?;
    let names: BTreeSet<_> = expected.keys().copied().chain([MANIFEST]).collect();
    if actual.iter().map(String::as_str).collect::<BTreeSet<_>>() != names {
        return Err(failure(
            "The downloaded file set does not match its authenticated manifest",
        ));
    }
    for (name, digest) in expected {
        if digest != &sha256_file(&root.join(name))? {
            return Err(failure("A tested development artifact digest changed"));
        }
    }
    Ok(())
}

fn zip_members<R: Read + Seek>(archive: &ZipArchive<R>) -> Result<BTreeSet<String>> {
    if archive.has_duplicate_names() {
        return Err(failure(
            "The downloaded source ZIP contains duplicate members",
        ));
    }
    let names = archive
        .file_names()
        .map(|name| name.map(|name| name.into_owned()))
        .collect::<std::result::Result<BTreeSet<_>, _>>()?;
    if names.len() != archive.len() {
        return Err(failure(
            "The downloaded source ZIP contains duplicate members",
        ));
    }
    Ok(names)
}

fn member_equal(
    mut first: impl Read,
    mut second: impl Read,
    mut remaining: u64,
) -> io::Result<bool> {
    let mut left = [0; 16_384];
    let mut right = [0; 16_384];
    while remaining > 0 {
        let size = remaining.min(left.len() as u64) as usize;
        first.read_exact(&mut left[..size])?;
        second.read_exact(&mut right[..size])?;
        if left[..size] != right[..size] {
            return Ok(false);
        }
        remaining -= size as u64;
    }
    Ok(first.read(&mut left[..1])? == 0 && second.read(&mut right[..1])? == 0)
}

fn expected_proof(name: &str, host: Host) -> bool {
    let host = host.name().to_ascii_lowercase();
    name.strip_prefix("docs/evidence/chromium-source-export-")
        .and_then(|name| name.strip_suffix(&format!("-{host}.json")))
        .is_some_and(|run| !run.is_empty() && run.bytes().all(|byte| byte.is_ascii_digit()))
}

fn verify_source(
    root: &Path,
    repository: &Path,
    selection: Selection<'_>,
    manifest: &Value,
) -> Result<()> {
    let scratch = tempfile::tempdir()?;
    let expected_name = scratch.path().join("expected-source.zip");
    process::stdout(
        process::command("git", &process::build_environment())
            .current_dir(repository)
            .args(["archive", "--format=zip", "--output"])
            .arg(&expected_name)
            .arg(selection.revision),
        Duration::from_secs(60),
        1024 * 1024,
    )?;
    let mut reference = ZipArchive::new(BufReader::new(File::open(expected_name)?))?;
    let mut source = ZipArchive::new(BufReader::new(regular_file(&root.join(SOURCE))?))?;
    let names = zip_members(&reference)?;
    let mismatch =
        || failure("The downloaded first-party source differs from the exact build commit");
    if zip_members(&source)? != names {
        return Err(mismatch());
    }
    let proof_bytes = read_bounded(&root.join(PROOF), MAX_METADATA)?;
    let mut tracked_proof = false;
    for name in names {
        let expected = reference.by_name(&name)?;
        let actual = source.by_name(&name)?;
        let size = expected.size();
        if actual.size() != size || !member_equal(actual, expected, size)? {
            return Err(mismatch());
        }
        if !tracked_proof
            && expected_proof(&name, selection.host)
            && size == proof_bytes.len() as u64
        {
            let expected = reference.by_name(&name)?;
            tracked_proof = member_equal(expected, proof_bytes.as_slice(), size)?;
        }
    }
    let proof: Value = serde_json::from_slice(&proof_bytes)?;
    if !tracked_proof
        || proof["host"] != selection.host.name()
        || proof["arch"] != "x86-64"
        || proof["source_archive"] != manifest["upstream_source"]
        || !proof["source_archive"].is_object()
        || proof["chromium_revision"]
            .as_str()
            .is_none_or(|revision| manifest["chromium_revision"] != revision)
        || proof["chromium_version"]
            .as_str()
            .is_none_or(|version| manifest["chromium_version"] != version)
    {
        return Err(failure(
            "The verified upstream source differs from the exact build commit",
        ));
    }
    Ok(())
}

pub(super) fn verify_subjects(
    root: &Path,
    repository: &Path,
    selection: Selection<'_>,
) -> Result<()> {
    let manifest = identity(root, selection)?;
    verify_contents(root, &manifest)?;
    verify_source(root, repository, selection, &manifest)
}

fn proof_bundle(root: &Path) -> Result<PathBuf> {
    let mut result = None;
    for entry in root.read_dir()? {
        let entry = entry?;
        let path = entry.path();
        if matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("json" | "jsonl")
        ) && !entry.file_type()?.is_symlink()
            && checked_metadata(&path)?.is_file()
            && result.replace(path).is_some()
        {
            return Err(failure("Expected one retained offline attestation bundle"));
        }
    }
    result.ok_or_else(|| failure("Expected one retained offline attestation bundle"))
}

pub(super) fn verify_download(
    root: &Path,
    provenance: &Path,
    repository: &Path,
    selection: Selection<'_>,
    mut attest: impl FnMut(&mut Command) -> Result<()>,
) -> Result<Value> {
    let bundle = proof_bundle(provenance)?;
    let manifest = identity(root, selection)?;
    for name in SUBJECTS {
        let subject = root.join(name);
        drop(regular_file(&subject)?);
        attest(
            Command::new("gh")
                .args(["attestation", "verify"])
                .arg(subject)
                .args(["--repo", REPOSITORY, "--bundle"])
                .arg(&bundle)
                .args([
                    "--source-ref",
                    "refs/heads/main",
                    "--source-digest",
                    selection.revision,
                    "--signer-workflow",
                    SIGNER,
                    "--deny-self-hosted-runners",
                ]),
        )?;
    }
    verify_contents(root, &manifest)?;
    verify_source(root, repository, selection, &manifest)?;
    Ok(manifest)
}
