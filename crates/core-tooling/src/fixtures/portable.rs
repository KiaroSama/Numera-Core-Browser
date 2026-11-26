//! Measure the native GN runtime, archive it, and test the extracted files.
use super::{
    Environment, Result, install_linux_sandbox, smoke_browser_ownership, smoke_driver,
    smoke_profile_data, smoke_renderer, support::*,
};
use flate2::{Compression, bufread::GzDecoder, write::GzEncoder};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, BufReader},
    path::{Path, PathBuf},
};

const RUNTIME_ARCHIVE: &str = "numera-portable-runtime.tar.gz";

pub fn runtime_dependency_path(name: &str) -> Result<String> {
    let parts: Vec<_> = name
        .split('/')
        .filter(|part| !matches!(*part, "" | "."))
        .collect();
    if name.starts_with('/')
        || parts.is_empty()
        || parts.contains(&"..")
        || name.contains(['\\', ':'])
        || name.chars().any(|ch| ch < ' ')
    {
        return Err(failure(
            "GN runtime dependency is outside the owned build directory",
        ));
    }
    Ok(parts.join("/"))
}

#[derive(Serialize)]
struct Layout {
    binary: &'static str,
    driver: &'static str,
    required: BTreeSet<&'static str>,
    dependencies: BTreeSet<String>,
    gn_sha256: String,
    gn_targets: [&'static str; 2],
}

fn describe(source: &Path, host: &str, environment: &Environment) -> Result<Layout> {
    let source = absolute(source)?;
    let (binary, driver, gn, target, intermediates) = match host {
        "Windows" => (
            "chrome.exe",
            "chromedriver.exe",
            "buildtools/win/gn.exe",
            "//chrome/test/chromedriver:chromedriver_server",
            &["initialexe/chrome.exe", "initialexe/chrome.exe.pdb"][..],
        ),
        "Linux" => (
            "chrome",
            "chromedriver",
            "buildtools/linux64/gn",
            "//chrome/test/chromedriver:chromedriver_server.unstripped",
            &["chromedriver.unstripped"][..],
        ),
        _ => return Err(failure("Expected a native Windows or Linux Chromium build")),
    };
    let gn = source.join(gn);
    let targets = ["//chrome:chrome_initial", target];
    let mut required = BTreeSet::from([binary, driver]);
    if host == "Linux" {
        required.insert("chrome_sandbox");
    }
    let mut dependencies: BTreeSet<_> = required.iter().map(|name| (*name).to_owned()).collect();
    for target in targets {
        let output = capture_success(
            command(&gn, environment)
                .args(["desc", "out/Numera", target, "runtime_deps"])
                .current_dir(&source),
            90,
            1024 * 1024,
        )?;
        if output.is_empty() {
            return Err(failure("Unexpected GN runtime dependency list size"));
        }
        for line in std::str::from_utf8(&output)?
            .lines()
            .filter(|line| !line.is_empty())
        {
            let name = runtime_dependency_path(line)?;
            if !intermediates.contains(&name.as_str()) {
                dependencies.insert(name);
            }
        }
    }
    Ok(Layout {
        binary,
        driver,
        required,
        dependencies,
        gn_sha256: sha256(&gn)?,
        gn_targets: targets,
    })
}

pub fn describe_portable_runtime(
    source: &Path,
    host: &str,
    environment: &Environment,
) -> Result<Value> {
    Ok(serde_json::to_value(describe(source, host, environment)?)?)
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}

fn path_metadata(path: &Path, why: &str) -> Result<fs::Metadata> {
    let metadata = path.symlink_metadata()?;
    if is_link(&metadata) {
        return Err(failure(why));
    }
    Ok(metadata)
}

fn no_link_prefix(path: &Path, why: &str) -> Result<()> {
    let mut prefix = PathBuf::new();
    for part in path.components() {
        prefix.push(part);
        if matches!(part, std::path::Component::Normal(_)) {
            path_metadata(&prefix, why)?;
        }
    }
    Ok(())
}

fn collect_files(build: &Path, path: &Path, files: &mut BTreeSet<String>) -> Result<()> {
    let metadata = path_metadata(path, "Runtime directory contains an unexpected link")?;
    if metadata.is_dir() {
        for entry in path.read_dir()? {
            collect_files(build, &entry?.path(), files)?;
        }
    } else if metadata.is_file() {
        let relative = path.strip_prefix(build)?;
        let components = relative.iter().map(utf8).collect::<Result<Vec<_>>>()?;
        files.insert(runtime_dependency_path(&components.join("/"))?);
    } else {
        return Err(failure("Runtime dependency is not a regular file"));
    }
    Ok(())
}

fn resolve_runtime_files(
    build: &Path,
    dependencies: &BTreeSet<String>,
    host: &str,
) -> Result<BTreeSet<String>> {
    let mut files = BTreeSet::new();
    for name in dependencies {
        let name = runtime_dependency_path(name)?;
        let mut path = build.to_path_buf();
        for part in name.split('/') {
            path.push(part);
            path_metadata(&path, "GN runtime dependency follows an unexpected link")?;
        }
        collect_files(build, &path, &mut files)?;
    }
    if host == "Windows" {
        // PDBs are debugger symbols. Chromium loads the DLLs and resources without them.
        files.retain(|name| {
            !Path::new(name)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdb"))
        });
    }
    let mut spellings = BTreeMap::new();
    for name in &files {
        let mut prefix = name.as_str();
        loop {
            if spellings
                .insert(prefix.to_uppercase(), prefix)
                .is_some_and(|old| old != prefix)
            {
                return Err(failure(
                    "Runtime dependency names conflict on a case-insensitive filesystem",
                ));
            }
            let Some((parent, _)) = prefix.rsplit_once('/') else {
                break;
            };
            prefix = parent;
        }
    }
    Ok(files)
}

#[derive(Serialize, Deserialize)]
struct FileRecord {
    sha256: String,
    bytes: u64,
}

#[derive(Serialize, Deserialize)]
struct Inventory {
    files: BTreeMap<String, FileRecord>,
    bytes: u64,
    gn_sha256: String,
    gn_targets: [String; 2],
    publisher_signing: String,
}

fn measure(
    source: &Path,
    output: &Path,
    host: &str,
    environment: &Environment,
) -> Result<(PathBuf, Layout, Inventory)> {
    let build = absolute(&source.join("out/Numera"))?;
    no_link_prefix(&build, "Chromium build output follows an unexpected link")?;
    let layout = describe(source, host, environment)?;
    let paths = resolve_runtime_files(&build, &layout.dependencies, host)?;
    if layout.required.iter().any(|name| !paths.contains(*name)) {
        return Err(failure(
            "Runtime dependency list is missing required executable files",
        ));
    }
    let mut files = BTreeMap::new();
    let mut bytes = 0_u64;
    for name in paths {
        let file = build.join(&name);
        let length = file.metadata()?.len();
        bytes = bytes
            .checked_add(length)
            .ok_or_else(|| failure("Runtime inventory size overflow"))?;
        files.insert(
            name,
            FileRecord {
                sha256: sha256(&file)?,
                bytes: length,
            },
        );
    }
    let inventory = Inventory {
        files,
        bytes,
        gn_sha256: layout.gn_sha256.clone(),
        gn_targets: layout.gn_targets.map(str::to_owned),
        publisher_signing: "NOT_CONFIGURED".to_owned(),
    };
    write_json(&output.join("portable-runtime-inventory.json"), &inventory)?;
    Ok((build, layout, inventory))
}

#[cfg(unix)]
fn disk_free(path: &Path) -> io::Result<u64> {
    let stats = rustix::fs::statvfs(path)?;
    Ok(stats.f_bavail.saturating_mul(stats.f_frsize))
}

#[cfg(windows)]
#[expect(unsafe_code, reason = "Windows disk availability is a native OS query")]
fn disk_free(path: &Path) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let encoded = path
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect::<Vec<_>>();
    let mut available = 0;
    // SAFETY: filename storage is NUL terminated and available is writable.
    if unsafe {
        GetDiskFreeSpaceExW(
            encoded.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(available)
}

#[cfg(unix)]
fn file_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn file_mode(metadata: &fs::Metadata) -> u32 {
    if metadata.permissions().readonly() {
        0o444
    } else {
        0o666
    }
}

fn write_archive(build: &Path, package: &Path, inventory: &Inventory) -> Result<()> {
    let encoder = GzEncoder::new(File::create_new(package)?, Compression::new(3));
    let mut archive = tar::Builder::new(encoder);
    for (name, record) in &inventory.files {
        let path = build.join(name);
        let mut file = File::open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() != record.bytes {
            return Err(failure("Runtime file identity changed during packaging"));
        }
        let name = format!("runtime/{name}");
        let mut header = tar::Header::new_ustar();
        header.set_size(record.bytes);
        header.set_mode(file_mode(&metadata));
        if !name.is_ascii() || name.len() > 100 {
            archive.append_pax_extensions([("path", name.as_bytes())])?;
            archive.append_data(&mut header, "runtime/pax-path", &mut file)?;
        } else {
            archive.append_data(&mut header, name, &mut file)?;
        }
    }
    archive.into_inner()?.finish()?;
    Ok(())
}

fn extract_archive(package: &Path, destination: &Path, inventory: &Inventory) -> Result<()> {
    fs::create_dir(destination)?;
    let mut archive = tar::Archive::new(GzDecoder::new(BufReader::new(File::open(package)?)));
    let mut seen = BTreeSet::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let raw = entry.path_bytes();
        let name = std::str::from_utf8(&raw)?
            .strip_prefix("runtime/")
            .ok_or_else(|| {
                failure("Portable archive members differ from the staged file inventory")
            })?
            .to_owned();
        let mode = entry.header().mode()?;
        let record = inventory
            .files
            .get(&name)
            .filter(|record| {
                entry.header().entry_type().is_file()
                    && mode & 0o7000 == 0
                    && record.bytes == entry.size()
                    && seen.insert(name.clone())
            })
            .ok_or_else(|| {
                failure("Portable archive members differ from the staged file inventory")
            })?;
        let path = destination.join("runtime").join(&name);
        let parent = path
            .parent()
            .ok_or_else(|| failure("Missing extracted runtime parent"))?;
        fs::create_dir_all(parent)?;
        let mut file = File::create_new(&path)?;
        if io::copy(&mut entry, &mut file)? != record.bytes {
            return Err(failure(
                "Extracted runtime is not byte-identical to the staged files",
            ));
        }
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let safe_mode = if mode & 0o100 == 0 {
                mode & 0o644
            } else {
                mode & 0o755
            };
            fs::set_permissions(&path, fs::Permissions::from_mode(safe_mode | 0o600))?;
        }
        if sha256(&path)? != record.sha256 {
            return Err(failure(
                "Extracted runtime is not byte-identical to the staged files",
            ));
        }
    }
    if seen.len() != inventory.files.len() {
        return Err(failure(
            "Portable archive members differ from the staged file inventory",
        ));
    }
    Ok(())
}

fn verify_portable_archive(
    package: &Path,
    archive: &Value,
    inventory: &Inventory,
    host: &str,
    environment: &Environment,
) -> Result<Value> {
    let name = if host == "Windows" {
        "numera-core.exe"
    } else {
        "numera-core"
    };
    let binary = fs::canonicalize(Path::new("target/release").join(name))?;
    let digest = archive["sha256"]
        .as_str()
        .ok_or_else(|| failure("Missing runtime archive digest"))?;
    let output = capture_success(
        command(&binary, environment)
            .args(["--json", "engines", "verify-archive"])
            .arg(package)
            .arg(digest),
        300,
        65_536,
    )?;
    let mut report: Value = serde_json::from_slice(&output)?;
    if report
        != json!({"archive_verified":true,"archive_sha256":digest,
        "archive_bytes":archive["bytes"],"files":archive["files"],
        "expanded_file_bytes":inventory.bytes,"approved_package":false})
    {
        return Err(failure(
            "The native CLI did not confirm the exact tested runtime archive",
        ));
    }
    report["result"] = json!("PASS");
    report["cli_sha256"] = json!(sha256(&binary)?);
    Ok(report)
}

pub fn smoke_portable_runtime(
    source: &Path,
    scratch: &Path,
    output: &Path,
    host: &str,
    environment: &Environment,
) -> Result<Value> {
    let (build, layout, inventory) = measure(source, output, host, environment)?;
    let reserve = inventory
        .bytes
        .checked_add(64 * 1024 * 1024)
        .ok_or_else(|| failure("Runtime archive size overflow"))?;
    if disk_free(scratch)? < reserve {
        return Err(failure("Insufficient space to write the portable archive"));
    }
    let package = absolute(&scratch.join(RUNTIME_ARCHIVE))?;
    write_archive(&build, &package, &inventory)?;
    if disk_free(scratch)? < reserve {
        return Err(failure(
            "Insufficient space to extract the portable archive",
        ));
    }
    let destination = absolute(&scratch.join("Numera extracted مسیر"))?;
    extract_archive(&package, &destination, &inventory)?;
    let archive = json!({"format":"tar.gz","files":inventory.files.len(),
        "bytes":package.metadata()?.len(),"sha256":sha256(&package)?});
    let verified = verify_portable_archive(&package, &archive, &inventory, host, environment)?;
    let held = build.with_file_name("Numera-fixture-held");
    if held.symlink_metadata().is_ok() {
        return Err(failure("Reserved source build path already exists"));
    }
    let logs = output.join("portable-runtime");
    fs::create_dir(&logs)?;
    let profile = destination.join("داده‌های مرورگر");
    fs::create_dir(&profile)?;
    let runtime = destination.join("runtime");
    let mut relocated = environment.clone();
    let helper = if host == "Linux" {
        // Chromium's SUID launcher interprets spaces as argument separators.
        let helper_directory = absolute(&scratch.join("portable-sandbox"))?;
        fs::create_dir(&helper_directory)?;
        let mut report = install_linux_sandbox(&runtime, &helper_directory, &mut relocated)?;
        report["staged_source"] = json!("chrome_sandbox");
        report["fixture_location"] = json!("Separate space-free privileged fixture directory");
        Some(report)
    } else {
        None
    };
    fs::rename(&build, &held)?;
    let result = (|| -> Result<(Value, Value, Value)> {
        let browser = runtime.join(layout.binary);
        let driver = runtime.join(layout.driver);
        smoke_renderer(&browser, &profile, &logs, &relocated)?;
        let webdriver = smoke_driver(&browser, &driver, &profile, &logs, &relocated, false)?;
        let ownership = smoke_browser_ownership(&browser, &destination, &logs, &relocated)?;
        let data = smoke_profile_data(&browser, &driver, &profile, &logs, &relocated)?;
        Ok((webdriver, ownership, data))
    })();
    fs::rename(held, build)?;
    let (webdriver, ownership, data) = result?;
    let mut report = json!({"result":"PASS",
        "scope":"Extracted runtime, controlled rendering, WebDriver and native process ownership",
        "archive":archive,"runtime_inventory_sha256":sha256(&output.join("portable-runtime-inventory.json"))?,
        "webdriver":webdriver,"archive_verification":verified,"process_ownership":ownership,"profile_data":data});
    if let Some(helper) = helper {
        report["sandbox_helper"] = helper;
    }
    Ok(report)
}

#[cfg(test)]
mod tests;
