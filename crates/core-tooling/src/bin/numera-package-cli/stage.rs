use crate::common::{
    Arguments, Result, UsageError, canonical_json, capture, checked, git_head, git_tree, hash_file,
    info, lexical_relative,
};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use zip::ZipArchive;

const ROOT: &str = "development-cli";
const SOURCE: &str = "complete-source.zip";

pub fn run(args: Arguments) -> Result<PathBuf> {
    let target = args.path("--target")?;
    let runner = target.to_str().ok_or("Invalid package target")?;
    let (system, machine) = match (env::consts::OS, env::consts::ARCH, runner) {
        ("linux", "x86_64", "ubuntu-24.04") => ("Linux", "x86_64"),
        ("windows", "x86_64", "windows-2025") => ("Windows", "AMD64"),
        _ => {
            return Err(UsageError(
                "Package target must match the native Windows or Linux x86-64 host",
            )
            .into());
        }
    };

    let repo = env::current_dir()?.canonicalize()?;
    let binary = args.path("--binary")?.canonicalize()?;
    let launcher = args
        .optional_path("--launcher")
        .map(fs::canonicalize)
        .transpose()?;
    let launcher_name = if cfg!(windows) {
        "numera-core-gui.exe"
    } else {
        "numera-core-gui"
    };
    if !binary.is_file() || !binary.starts_with(repo.join("target")) {
        return Err("Select a compiled workspace CLI".into());
    }
    if launcher.as_ref().is_some_and(|launcher| {
        launcher.file_name() != Some(OsStr::new(launcher_name))
            || !launcher.is_file()
            || !launcher.starts_with(repo.join("target"))
    }) {
        return Err("Select a compiled workspace desktop launcher".into());
    }

    checked(
        Command::new("git").args(["diff", "--exit-code", "HEAD", "--"]),
        30,
    )?;
    let commit = git_head()?;
    let index = checked(Command::new("git").args(["ls-files", "--stage", "-z"]), 30)?;
    if index
        .stdout
        .split(|byte| *byte == 0)
        .any(|item| item.starts_with(b"160000 "))
    {
        return Err("Complete source packaging does not support Git submodules".into());
    }
    let tracked = git_tree(&commit, &[])?;
    if tracked
        .iter()
        .any(|name| matches!(name.split('/').next(), Some(".ai" | ".codex" | ".agents")))
    {
        return Err("Private assistant files cannot be distributed".into());
    }
    let output = output_path(&repo, &args.path("--output")?)?;
    let staged = output.join(binary.file_name().ok_or("Missing CLI input filename")?);
    let distributable = archive_path(&output)?;
    if distributable.exists() {
        return Err("Development package archive already exists".into());
    }
    fs::create_dir_all(output.parent().ok_or("Missing package parent")?)?;
    fs::create_dir(&output)?;
    fs::copy(&binary, &staged)?;
    if let Some(launcher) = &launcher {
        fs::copy(launcher, output.join(launcher_name))?;
    }

    let source = output.join(SOURCE);
    checked(
        Command::new("git")
            .args(["archive", "--format=zip", "--output"])
            .arg(&source)
            .arg(&commit),
        60,
    )?;
    copy_archived_metadata(&source, &output, &tracked)?;

    let mut command = headless_command(&staged);
    let doctor = checked(command.args(["--json", "doctor"]), 15)?;
    let doctor: Value = serde_json::from_slice(&doctor.stdout)?;
    if ["owned_engine_available", "python_required", "node_required"]
        .into_iter()
        .any(|key| doctor.get(key) != Some(&Value::Bool(false)))
    {
        return Err("Development package reported unexpected runtime capabilities".into());
    }
    let version = checked(
        Command::new(&staged).arg("--version").current_dir(&output),
        15,
    )?;
    let version: Value = serde_json::from_slice(&version.stdout)?;
    let executable_hash = hash_file(&staged)?;
    let mut manifest = json!({
        "schema_revision": 1,
        "product": "Numera Core Browser development CLI",
        "channel": "development-control-plane",
        "version": version.get("version").ok_or("Missing native CLI version")?,
        "source_commit": commit,
        "runner_target": runner,
        "native_system": system,
        "native_machine": machine,
        "executable": staged.file_name().and_then(OsStr::to_str).ok_or("Invalid executable filename")?,
        "executable_sha256": executable_hash,
        "complete_source": {
            "file": SOURCE,
            "sha256": hash_file(&source)?,
            "scope": "Complete first-party repository tree; independent third-party source locations and notices are in licenses/dependencies.json"
        },
        "engine_ready": false,
        "browser_gui": false,
        "archive_signature": "none",
        "publisher_signing": "NOT_CONFIGURED",
        "platform_signing": "NOT_EVALUATED",
        "clean_machine_installation": "NOT_RUN",
        "native_staged_doctor": "PASS"
    });
    let launcher_hash = if launcher.is_some() {
        let staged_launcher = output.join(launcher_name);
        verify_launcher(&staged_launcher)?;
        let digest = hash_file(&staged_launcher)?;
        manifest["desktop_launcher"] = json!({"executable": launcher_name, "sha256": digest});
        Some(digest)
    } else {
        None
    };
    fs::write(
        output.join("development-manifest.json"),
        canonical_json(manifest)?,
    )?;
    write_readme(&output, launcher_hash.is_some())?;

    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&distributable)?;
    let mut archive = tar::Builder::new(GzEncoder::new(file, Compression::default()));
    archive.follow_symlinks(false);
    archive.append_dir_all(ROOT, &output)?;
    archive.into_inner()?.finish()?;

    let extracted = tempfile::Builder::new()
        .prefix("Numera CLI فارسی ")
        .tempdir()?;
    extract_cli_archive(&distributable, extracted.path())?;
    let packaged = extracted.path().join(ROOT);
    let executable = packaged.join(staged.file_name().ok_or("Missing staged CLI filename")?);
    let doctor = checked(headless_command(&executable).args(["--json", "doctor"]), 15)?;
    let doctor: Value = serde_json::from_slice(&doctor.stdout)?;
    if doctor["owned_engine_available"] != Value::Bool(false)
        || hash_file(&executable)? != executable_hash
    {
        return Err("Extracted development CLI did not match the native staged executable".into());
    }
    if let Some(expected) = launcher_hash {
        let extracted_launcher = packaged.join(launcher_name);
        if hash_file(&extracted_launcher)? != expected {
            return Err("Extracted desktop launcher hash does not match the package".into());
        }
        verify_launcher(&extracted_launcher)?;
    }
    verify_archive_cli(&executable, extracted.path())?;
    info(
        "PASS: extracted native development CLI with matching source/notices; browser readiness remains false",
    );
    Ok(distributable)
}

fn output_path(repo: &Path, path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    if absolute
        .components()
        .any(|part| part == std::path::Component::ParentDir)
    {
        return Err("Package output must be a new workspace build directory".into());
    }
    for ancestor in absolute.ancestors() {
        match ancestor.canonicalize() {
            Ok(existing) => {
                let relative = absolute.strip_prefix(ancestor)?;
                let destination = existing.join(relative);
                if destination.starts_with(repo.join("build")) && !relative.as_os_str().is_empty() {
                    return Ok(destination);
                }
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err("Package output must be a new workspace build directory".into())
}

fn archive_path(output: &Path) -> Result<PathBuf> {
    let mut name = output
        .file_name()
        .ok_or("Missing output directory name")?
        .to_os_string();
    name.push(".tar.gz");
    Ok(output.with_file_name(name))
}

fn packaged_metadata(name: &str) -> bool {
    matches!(
        name,
        "LICENSE" | "THIRD_PARTY_NOTICES" | "Cargo.lock" | "licenses/dependencies.json"
    ) || name.starts_with("contracts/")
        || name.starts_with("licenses/dependencies/")
}

fn copy_archived_metadata(source: &Path, output: &Path, tracked: &BTreeSet<String>) -> Result<()> {
    let mut archive = ZipArchive::new(File::open(source)?)?;
    let mut actual = BTreeSet::new();
    for index in 0..archive.len() {
        let mut member = archive.by_index(index)?;
        if member.is_dir() {
            continue;
        }
        let name = member.name()?.into_owned();
        if !tracked.contains(&name) || !actual.insert(name.clone()) {
            return Err("Source archive omitted or added tracked files".into());
        }
        if !packaged_metadata(&name) {
            continue;
        }
        if !lexical_relative(&name)
            || member
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err("Unsafe runtime notice or contract path".into());
        }
        let dest = output.join(name);
        fs::create_dir_all(dest.parent().ok_or("Package data parent missing")?)?;
        let mut file = OpenOptions::new().write(true).create_new(true).open(dest)?;
        io::copy(&mut member, &mut file)?;
    }
    if &actual != tracked {
        return Err("Source archive omitted or added tracked files".into());
    }
    Ok(())
}

fn extract_cli_archive(archive: &Path, destination: &Path) -> Result<()> {
    let mut tar = tar::Archive::new(GzDecoder::new(File::open(archive)?));
    let mut names = BTreeSet::new();
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let name = path
            .to_str()
            .ok_or("Invalid development archive member name")?;
        let kind = entry.header().entry_type();
        if !lexical_relative(name.trim_end_matches('/'))
            || !path.starts_with(ROOT)
            || !names.insert(path.clone())
            || !(kind.is_file() || kind.is_dir())
            || !entry.unpack_in(destination)?
        {
            return Err("Invalid development archive member".into());
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub fn archive_names_and_source(path: &Path) -> Result<(BTreeSet<String>, File)> {
    let mut names = BTreeSet::new();
    let mut source = tempfile::tempfile()?;
    let mut tar = tar::Archive::new(GzDecoder::new(File::open(path)?));
    for member in tar.entries()? {
        let mut member = member?;
        let path = member
            .path()?
            .to_str()
            .ok_or("Invalid development archive member")?
            .replace('\\', "/");
        if !names.insert(path.clone()) {
            return Err("Duplicate development archive member".into());
        }
        if path == format!("{ROOT}/{SOURCE}") {
            io::copy(&mut member, &mut source)?;
        }
    }
    if source.metadata()?.len() == 0 {
        return Err("Development archive source ZIP is missing".into());
    }
    use std::io::Seek as _;
    source.rewind()?;
    Ok((names, source))
}

fn headless_command(executable: &Path) -> Command {
    let environment: BTreeMap<OsString, OsString> = env::vars_os()
        .filter(|(key, _)| {
            !["PATH", "DISPLAY", "WAYLAND_DISPLAY", "LD_LIBRARY_PATH"]
                .iter()
                .any(|name| key.eq_ignore_ascii_case(name))
        })
        .collect();
    let mut command = Command::new(executable);
    command.env_clear().envs(environment).env("PATH", "");
    if let Some(parent) = executable.parent() {
        command.current_dir(parent);
    }
    command
}

fn verify_launcher(executable: &Path) -> Result<()> {
    let help = checked(headless_command(executable).arg("--help"), 15)?;
    if serde_json::from_slice::<Value>(&help.stdout)?
        != json!({"usage": "numera-core-gui STORE DATA_ROOT PROFILE [URL]"})
    {
        return Err("The development desktop launcher returned unexpected usage".into());
    }
    Ok(())
}

fn verify_archive_cli(executable: &Path, directory: &Path) -> Result<()> {
    const PAYLOAD: &[u8] = b"controlled portable archive data";
    let fixture = directory.join("بررسی بسته.tar.gz");
    let mut archive = tar::Builder::new(GzEncoder::new(
        File::create_new(&fixture)?,
        Compression::default(),
    ));
    archive.append_pax_extensions([("path", "runtime/locales/داده‌های مرورگر.pak".as_bytes())])?;
    let mut header = tar::Header::new_ustar();
    header.set_size(PAYLOAD.len() as u64);
    header.set_mode(0o644);
    archive.append_data(&mut header, "runtime/locales/fixture.pak", PAYLOAD)?;
    archive.into_inner()?.finish()?;

    let digest = hash_file(&fixture)?;
    let mut command = headless_command(executable);
    let result = checked(
        command
            .args(["--json", "engines", "verify-archive"])
            .arg(&fixture)
            .arg(&digest),
        15,
    )?;
    if serde_json::from_slice::<Value>(&result.stdout)?
        != json!({
            "archive_verified": true, "archive_sha256": digest, "archive_bytes": fixture.metadata()?.len(),
            "files": 1, "expanded_file_bytes": PAYLOAD.len(), "approved_package": false,
        })
    {
        return Err("The extracted CLI returned unexpected runtime archive evidence".into());
    }
    let mut command = headless_command(executable);
    let mismatch = capture(
        command
            .args(["--json", "engines", "verify-archive"])
            .arg(&fixture)
            .arg("0".repeat(64)),
        None,
        Duration::from_secs(15),
    )?;
    if mismatch.status.code() != Some(2)
        || serde_json::from_slice::<Value>(&mismatch.stdout)?["error"]["message"]
            != "Runtime archive SHA-256 does not match"
        || hash_file(&fixture)? != digest
        || directory.join("runtime").exists()
        || executable
            .parent()
            .is_some_and(|path| path.join("runtime").exists())
    {
        return Err("The extracted CLI did not enforce read-only archive integrity checks".into());
    }
    Ok(())
}

fn write_readme(output: &Path, launcher: bool) -> Result<()> {
    let launcher_note = if launcher {
        "The optional numera-core-gui launcher requires a separately installed, signed Chromium package.\n"
    } else {
        ""
    };
    fs::write(
        output.join("README.txt"),
        format!(
            "Numera Core Browser — development control-plane CLI\n\n\
             Run ./numera-core --json doctor (PowerShell: .\\numera-core.exe --json doctor) from this directory.\n\
             The CLI provides protocol diagnostics, launch planning and signed package administration.\n\
             Install the Chromium engine separately.\n\
             {launcher_note}\
             This is unsigned development software, not an integration-ready or production browser release.\n\
             Native execution was tested on the named CI target; clean-machine OS/CRT prerequisites are not certified.\n\
             Complete matching first-party source is in complete-source.zip. Read its README for pinned build instructions.\n\
             LICENSE and third-party notices apply; commercial use is not granted by the custom project license.\n"
        ),
    )?;
    Ok(())
}
