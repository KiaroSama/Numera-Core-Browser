//! Bind the exact tested archive to its source lock, notices and original tarball.
use super::{Environment, Result, support::*};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

fn digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn digest(value: &str, characters: usize) -> bool {
    value.len() == characters
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn string(value: &Value) -> Result<&str> {
    value
        .as_str()
        .ok_or_else(|| failure("Missing development runtime evidence field"))
}

pub fn read_source_export(
    filename: &str,
    host: &str,
    lock_sha256: &str,
) -> Result<(PathBuf, Value)> {
    let name = filename
        .strip_prefix("chromium-source-export-")
        .and_then(|value| value.strip_suffix(".json"))
        .and_then(|value| value.rsplit_once('-'));
    if !name.is_some_and(|(version, host)| digits(version) && matches!(host, "linux" | "windows")) {
        return Err(failure(
            "Select a verified Chromium source export from docs/evidence",
        ));
    }
    let path = Path::new("docs/evidence").join(filename);
    let proof: Value = read_json(&path)?;
    let export = &proof["source_archive"];
    let checks = &proof["content_checks"];
    let version = string(&proof["chromium_version"])?;
    let generation = string(&export["generation"])?;
    let archive = format!(
        "https://commondatastorage.googleapis.com/chromium-browser-official/chromium-{version}.tar.xz"
    );
    if proof["host"] != host
        || proof["arch"] != "x86-64"
        || proof["source_lock_sha256"] != lock_sha256
        || proof["branding_patch_check"] != "PASS"
        || checks["deps_identical_to_pinned_git"] != true
        || checks["branding_matches_pinned_git"] != true
        || checks["version_matches"] != true
        || [
            "absent_checkout_directories",
            "ffmpeg_changed_files",
            "ffmpeg_omitted_regular_files",
        ]
        .iter()
        .any(|key| !checks[key].as_array().is_some_and(Vec::is_empty))
    {
        return Err(failure(
            "Verified source export does not match this native Chromium build",
        ));
    }
    let pieces = version.split('.').collect::<Vec<_>>();
    if pieces.len() != 4
        || !pieces.into_iter().all(digits)
        || !digits(generation)
        || !digest(string(&export["sha256"])?, 64)
        || export["bytes"].as_u64().unwrap_or(0) < 1
        || export["url"] != format!("{archive}?generation={generation}")
        || export["hashes_url"] != format!("{archive}.hashes")
    {
        return Err(failure(
            "Verified source export contains inconsistent upstream content metadata",
        ));
    }
    if host == "Windows"
        && proof["supplemental_git_archives"]
            .as_object()
            .is_none_or(serde_json::Map::is_empty)
    {
        return Err(failure(
            "Verified Windows source export must include platform checkout references",
        ));
    }
    Ok((path, proof))
}

fn copy_source_evidence(
    source: &Path,
    output: &Path,
    base: &Path,
    attachments: &mut BTreeMap<String, String>,
) -> Result<()> {
    fs::create_dir(output)?;
    for entry in source.read_dir()? {
        let entry = entry?;
        let name = entry.file_name();
        if utf8(&name)?.ends_with(".log") {
            continue;
        }
        let from = entry.path();
        let destination = output.join(name);
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_source_evidence(&from, &destination, base, attachments)?;
        } else if kind.is_file() {
            fs::copy(from, &destination)?;
            let name = destination
                .strip_prefix(base)?
                .iter()
                .map(utf8)
                .collect::<Result<Vec<_>>>()?
                .join("/");
            attachments.insert(name, sha256(&destination)?);
        } else {
            return Err(failure(
                "Linked source evidence contains an unexpected file type",
            ));
        }
    }
    Ok(())
}

pub fn retain_development_bundle(
    root: &Path,
    evidence: &Path,
    report: &Value,
    source_proof: &(PathBuf, Value),
    environment: &Environment,
) -> Result<Value> {
    let (proof_path, proof) = source_proof;
    let materials = &report["build_materials"];
    let portable = &report["portable_runtime"];
    let archive = &portable["archive"];
    if [
        portable,
        &portable["archive_verification"],
        &portable["process_ownership"],
        &portable["profile_data"],
        &report["automation_clients"],
        &report["headed_browser"],
    ]
    .iter()
    .any(|result| result["result"] != "PASS")
    {
        return Err(failure(
            "Development runtime retention requires passing browser validation evidence",
        ));
    }
    let source_materials = evidence.join("linked-build-materials");
    let git_inventory = source_materials.join("git-source-revisions.json");
    let inventory_digest = string(&proof["native_inventory"]["git_checkouts_sha256"])?;
    if materials["chromium_revision"] != proof["chromium_revision"]
        || materials["chromium_version"] != proof["chromium_version"]
        || materials["source_inventory"]["sha256"] != inventory_digest
        || sha256(&git_inventory)? != inventory_digest
        || materials["source_inventory"]["deps_sha256"]
            != proof["content_checks"]["verified_upstream_files_sha256"]["DEPS"]
        || portable["archive_verification"]["archive_sha256"] != archive["sha256"]
        || portable["archive_verification"]["archive_verified"] != true
    {
        return Err(failure(
            "The tested Chromium runtime and its retained source inputs differ",
        ));
    }
    if let Some(supplements) = proof["supplemental_git_archives"].as_object() {
        let inventory: Value = read_json(&git_inventory)?;
        for (name, dependency) in supplements {
            let checkout = &inventory["git_checkouts"][name];
            if !checkout.is_object()
                || ["url", "revision"]
                    .iter()
                    .any(|field| checkout[field] != dependency[field])
            {
                return Err(failure(
                    "The native checkout differs from its supplemental source reference",
                ));
            }
            let site = string(&checkout["url"])?;
            let url = format!(
                "{}/+archive/{}.tar.gz",
                site.strip_suffix(".git").unwrap_or(site),
                string(&checkout["revision"])?
            );
            if dependency["archive_url"] != url
                || dependency["archive_bytes"].as_u64().unwrap_or(0) == 0
                || !digest(string(&dependency["archive_sha256"])?, 64)
            {
                return Err(failure(
                    "A supplemental checkout archive has invalid content metadata",
                ));
            }
        }
    }

    let runtime_inventory = evidence.join("portable-runtime-inventory.json");
    if portable["runtime_inventory_sha256"] != sha256(&runtime_inventory)? {
        return Err(failure(
            "The published runtime inventory differs from the tested inventory",
        ));
    }
    let inventory: Value = read_json(&runtime_inventory)?;
    if inventory["files"].as_object().is_none_or(|files| {
        files
            .keys()
            .any(|name| name.to_lowercase().contains("widevine"))
    }) {
        return Err(failure(
            "The development bundle may only contain reviewed Chromium components",
        ));
    }
    let commit = string(&report["commit"])?;
    if !digest(commit, 40) {
        return Err(failure(
            "Invalid Numera source commit for the development bundle",
        ));
    }
    let run = string(&report["context"]["run_id"])?.parse::<u64>()?;
    let target = match report["host"].as_str() {
        Some("Linux") => "x86_64-unknown-linux-gnu",
        Some("Windows") => "x86_64-pc-windows-msvc",
        _ => return Err(failure("Expected a native Windows or Linux Chromium build")),
    };
    let archive_name = "numera-portable-runtime.tar.gz";
    let mut attachments = BTreeMap::new();
    let output = evidence.with_file_name("chromium-development-runtime");
    fs::create_dir(&output)?;
    copy_source_evidence(
        &source_materials,
        &output.join("source-evidence"),
        &output,
        &mut attachments,
    )?;
    for (original, name) in [
        (proof_path, "verified-upstream-source.json"),
        (&runtime_inventory, "runtime-inventory.json"),
    ] {
        let path = output.join(name);
        fs::copy(original, &path)?;
        attachments.insert(name.to_owned(), sha256(&path)?);
    }

    let source = output.join("numera-source.zip");
    if !timed(
        command("git", environment)
            .args(["archive", "--format=zip", "--output"])
            .arg(&source)
            .arg(commit),
        90,
    )?
    .success()
    {
        return Err(failure("Could not archive the matching Numera source"));
    }
    let runtime = output.join(archive_name);
    let scratch_archive = root.join(archive_name);
    if let Err(error) = fs::rename(&scratch_archive, &runtime) {
        if error.kind() != std::io::ErrorKind::CrossesDevices {
            return Err(error.into());
        }
        fs::copy(&scratch_archive, &runtime)?;
        fs::remove_file(scratch_archive)?;
    }
    let sha = sha256(&runtime)?;
    if runtime.metadata()?.len() != archive["bytes"].as_u64().unwrap_or(0)
        || archive["sha256"] != sha
    {
        return Err(failure(
            "The staged download does not match the tested Chromium runtime",
        ));
    }

    let mut runtime_subject = archive.clone();
    runtime_subject["file"] = json!(archive_name);
    let manifest = json!({
        "schema_revision":1,"package":"numera-chromium","build_mode":"development",
        "target":target,"chromium_version":materials["chromium_version"],"chromium_revision":materials["chromium_revision"],
        "numera_commit":commit,"patch_sha256":sha256(Path::new("upstream/chromium/patches/0001-product-name.patch"))?,
        "workflow_run":run,"runtime":runtime_subject,"numera_source":{"file":"numera-source.zip","sha256":sha256(&source)?},
        "upstream_source":proof["source_archive"],"source_evidence":"source-evidence",
        "attachments_sha256":attachments,"runtime_inventory_sha256":attachments["runtime-inventory.json"],
        "publisher_signature":"NOT_CONFIGURED","trusted_release":false,
        "validation":{"portable_runtime":"PASS","native_headed_browser":"PASS","browser_clients":"PASS",
            "rust_process_ownership":"PASS","profile_and_downloads":"PASS"},
        "linux_sandbox":portable["sandbox_helper"],"signed_application_updates":"NOT_RUN","clean_host_installation":"NOT_RUN"
    });
    let manifest_path = output.join("development-runtime.json");
    write_json(&manifest_path, &manifest)?;
    Ok(
        json!({"subdirectory":"chromium-development-runtime","manifest_sha256":sha256(&manifest_path)?,
        "runtime_sha256":sha,"trusted_release":false}),
    )
}
