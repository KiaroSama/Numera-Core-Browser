use super::distribution::{INVENTORY, RUNTIME, failure, read_bounded, valid_relative};
use core_tooling::{
    chromium_build::{cache::valid_sha256, metadata::Host, process},
    fixtures::{Environment, Result, install_linux_sandbox, smoke_headed_browser, smoke_renderer},
    sha256_file,
};
use flate2::bufread::GzDecoder;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::OsStr,
    fs::{self, File},
    io::{self, BufReader},
    path::{Path, PathBuf},
    time::Duration,
};

const DESKTOP_ENVIRONMENT: &[&str] = &[
    "PATH",
    "HOME",
    "LANG",
    "LC_ALL",
    "TMPDIR",
    "SYSTEMROOT",
    "WINDIR",
    "USERPROFILE",
    "LOCALAPPDATA",
    "APPDATA",
    "COMSPEC",
    "TEMP",
    "TMP",
    "SYSTEMDRIVE",
    "HOMEDRIVE",
    "HOMEPATH",
    "PROGRAMDATA",
];

#[derive(Deserialize)]
struct Inventory {
    bytes: u64,
    files: BTreeMap<String, Member>,
}

#[derive(Deserialize)]
struct Member {
    bytes: u64,
    sha256: String,
}

fn environment(keys: &[&str]) -> Environment {
    env::vars_os()
        .filter(|(key, _)| {
            keys.iter()
                .any(|name| key.eq_ignore_ascii_case(OsStr::new(name)))
        })
        .collect()
}

fn inventory(package: &Path, manifest: &Value) -> Result<Inventory> {
    let inventory: Inventory =
        serde_json::from_slice(&read_bounded(&package.join(INVENTORY), 64 * 1024 * 1024)?)?;
    let bytes = inventory
        .files
        .values()
        .try_fold(0u64, |total, item| total.checked_add(item.bytes));
    if bytes != Some(inventory.bytes)
        || manifest["runtime"]["files"].as_u64() != Some(inventory.files.len() as u64)
        || inventory.files.is_empty()
        || inventory
            .files
            .iter()
            .any(|(path, item)| !valid_relative(path) || !valid_sha256(&item.sha256))
    {
        return Err(failure(
            "The authenticated runtime inventory is inconsistent",
        ));
    }
    Ok(inventory)
}

fn native_verifier(
    binary: &Path,
    archive: &Path,
    manifest: &Value,
    inventory: &Inventory,
) -> Result<Value> {
    let digest = manifest["runtime"]["sha256"]
        .as_str()
        .filter(|value| valid_sha256(value))
        .ok_or_else(|| failure("The authenticated runtime digest is invalid"))?;
    let mut verifier_environment = environment(&["SYSTEMROOT"]);
    verifier_environment.insert("PATH".into(), "".into());
    let output = process::stdout(
        process::command(binary, &verifier_environment)
            .args(["--json", "engines", "verify-archive"])
            .arg(archive)
            .arg(digest),
        Duration::from_secs(300),
        1024 * 1024,
    )?;
    let info: Value = serde_json::from_slice(&output)?;
    if info
        != json!({"archive_verified":true,"archive_sha256":digest,
        "archive_bytes":manifest["runtime"]["bytes"],"files":manifest["runtime"]["files"],
        "expanded_file_bytes":inventory.bytes,"approved_package":false})
    {
        return Err(failure(
            "The native archive verifier did not confirm the tested download",
        ));
    }
    Ok(info)
}

fn extract(archive: &Path, destination: &Path, inventory: &Inventory) -> Result<PathBuf> {
    fs::create_dir(destination)?;
    let runtime = destination.join("runtime");
    fs::create_dir(&runtime)?;
    let mut tar = tar::Archive::new(GzDecoder::new(BufReader::new(File::open(archive)?)));
    let mut seen = BTreeSet::new();
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path_bytes();
        let name = std::str::from_utf8(&path)?
            .strip_prefix("runtime/")
            .filter(|path| valid_relative(path))
            .ok_or_else(|| failure("Unexpected runtime archive member name"))?
            .to_owned();
        let mode = entry.header().mode()?;
        let expected = inventory
            .files
            .get(&name)
            .filter(|record| {
                entry.header().entry_type().is_file()
                    && mode & 0o7000 == 0
                    && entry.size() == record.bytes
                    && seen.insert(name.clone())
            })
            .ok_or_else(|| {
                failure("The extracted runtime differs from the signed file inventory")
            })?;
        let path = runtime.join(name);
        let parent = path
            .parent()
            .ok_or_else(|| failure("Missing extracted runtime parent"))?;
        fs::create_dir_all(parent)?;
        let mut output = File::create_new(&path)?;
        if io::copy(&mut entry, &mut output)? != expected.bytes {
            return Err(failure(
                "The extracted runtime differs from the signed file inventory",
            ));
        }
        drop(output);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let safe = if mode & 0o100 != 0 {
                mode & 0o755
            } else {
                mode & 0o644
            };
            fs::set_permissions(&path, fs::Permissions::from_mode(safe | 0o600))?;
        }
        if sha256_file(&path)? != expected.sha256 {
            return Err(failure(
                "The extracted runtime differs from the signed file inventory",
            ));
        }
    }
    if seen.len() != inventory.files.len() {
        return Err(failure(
            "The extracted runtime differs from the signed file inventory",
        ));
    }
    Ok(runtime)
}

fn blocked_by_host_sandbox(path: &Path) -> bool {
    let Ok(log) = read_bounded(path, 4 * 1024 * 1024) else {
        return false;
    };
    [
        "No usable sandbox",
        "SUID sandbox helper binary was found, but is not configured correctly",
    ]
    .iter()
    .any(|message| {
        log.windows(message.len())
            .any(|bytes| bytes == message.as_bytes())
    })
}

pub(super) fn verify_and_smoke(
    package: &Path,
    manifest: &Value,
    scratch: &Path,
    output: &Path,
    report: &mut Value,
    host: Host,
) -> Result<()> {
    let archive = package.join(RUNTIME);
    let inventory = inventory(package, manifest)?;
    let binary = Path::new("target/release")
        .join(format!("numera-core{}", env::consts::EXE_SUFFIX))
        .canonicalize()?;
    report["archive_verification"] = native_verifier(&binary, &archive, manifest, &inventory)?;
    let runtime = extract(
        &archive,
        &scratch.join("Numera clean runtime مسیر"),
        &inventory,
    )?;
    report["extracted_file_hashes"] = json!("PASS");

    let browser = runtime.join(format!("chrome{}", env::consts::EXE_SUFFIX));
    let driver = runtime.join(format!("chromedriver{}", env::consts::EXE_SUFFIX));
    let mut environment = environment(DESKTOP_ENVIRONMENT);
    if host == Host::Linux {
        let no_admin = output.join("no-admin");
        let profile = scratch.join("no-admin-profile");
        fs::create_dir(&no_admin)?;
        fs::create_dir(&profile)?;
        match smoke_renderer(&browser, &profile, &no_admin, &environment) {
            Ok(()) => report["no_admin_renderer"] = json!("PASS"),
            Err(_) if blocked_by_host_sandbox(&no_admin.join("smoke-renderer.stderr")) => {
                report["no_admin_renderer"] = json!("BLOCKED_BY_HOST_SANDBOX");
                let helper = scratch.join("verified-sandbox-helper");
                fs::create_dir(&helper)?;
                report["sandbox_helper"] =
                    install_linux_sandbox(&runtime, &helper, &mut environment)?;
            }
            Err(error) => return Err(error),
        }
    } else {
        report["no_admin_renderer"] = json!("NOT_RUN");
    }
    let evidence = output.join("extracted-browser");
    let profile = scratch.join("پروفایل مستقل");
    fs::create_dir(&evidence)?;
    fs::create_dir(&profile)?;
    smoke_renderer(&browser, &profile, &evidence, &environment)?;
    report["extracted_renderer"] = json!("PASS");
    report["headful_browser"] =
        smoke_headed_browser(&browser, &driver, &profile, &evidence, &environment)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_tooling::sha256_hex;
    use flate2::{Compression, write::GzEncoder};

    fn make_archive(path: &Path) -> Inventory {
        let mut archive = tar::Builder::new(GzEncoder::new(
            File::create_new(path).unwrap(),
            Compression::fast(),
        ));
        let mut inventory = Inventory {
            bytes: 0,
            files: BTreeMap::new(),
        };
        for (name, bytes) in [
            ("chrome", b"synthetic browser bytes".as_slice()),
            ("locales/فارسی.pak", b"fixture bytes".as_slice()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            archive
                .append_data(&mut header, format!("runtime/{name}"), bytes)
                .unwrap();
            inventory.bytes += bytes.len() as u64;
            inventory.files.insert(
                name.to_owned(),
                Member {
                    bytes: bytes.len() as u64,
                    sha256: sha256_hex(bytes),
                },
            );
        }
        archive.into_inner().unwrap().finish().unwrap();
        inventory
    }

    #[test]
    fn extracts_only_signed_inventory_members_and_checks_their_bytes() {
        let work = tempfile::tempdir().unwrap();
        let file = work.path().join(RUNTIME);
        let mut inventory = make_archive(&file);
        let runtime = extract(
            &file,
            &work.path().join("Numera clean runtime مسیر"),
            &inventory,
        )
        .unwrap();
        assert_eq!(
            fs::read(runtime.join("locales/فارسی.pak")).unwrap(),
            b"fixture bytes"
        );
        inventory.files.get_mut("chrome").unwrap().sha256 = "0".repeat(64);
        assert!(extract(&file, &work.path().join("altered-inventory"), &inventory).is_err());
    }

    #[test]
    #[ignore = "requires NUMERA_CORE_TEST_BINARY pointing to the built Core executable"]
    fn calls_the_native_archive_verifier_with_a_synthetic_runtime() {
        let binary =
            PathBuf::from(env::var_os("NUMERA_CORE_TEST_BINARY").expect("Core executable"));
        let work = tempfile::tempdir().unwrap();
        let file = work.path().join(RUNTIME);
        let inventory = make_archive(&file);
        let manifest = json!({"runtime":{"file":RUNTIME,"sha256":sha256_file(&file).unwrap(),
            "bytes":file.metadata().unwrap().len(),"files":inventory.files.len()}});
        let report = native_verifier(&binary, &file, &manifest, &inventory).unwrap();
        assert_eq!(report["archive_verified"], true);
        assert_eq!(report["approved_package"], false);
    }
}
