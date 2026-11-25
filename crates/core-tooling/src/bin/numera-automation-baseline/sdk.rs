//! Execute a caller-supplied SDK runner; raw WebDriver is not SDK compatibility evidence.
//! The specification pins the executable and declared runner files. Transitive SDK
//! installation provenance remains the external runner's responsibility.
use super::{Result, digest, failure, read_json};
use core_tooling::fixtures::{
    Environment, bounded_child,
    webdriver::{Driver, fixture_url},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs::File,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PinnedFile {
    path: PathBuf,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Adapter {
    schema_revision: u32,
    program: PinnedFile,
    arguments: Vec<String>,
    files: Vec<PinnedFile>,
}

fn verify(file: &PinnedFile) -> Result<()> {
    if !file.path.is_absolute()
        || !std::fs::symlink_metadata(&file.path)?.is_file()
        || file.sha256.len() != 64
        || !file
            .sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        || digest(&file.path)? != file.sha256
    {
        return Err(failure("External SDK adapter file pin does not match"));
    }
    Ok(())
}

pub fn smoke_clients(
    binary: &Path,
    driver: &Path,
    scratch: &Path,
    output: &Path,
    environment: &Environment,
    adapter: Option<&Path>,
) -> Result<Value> {
    let Some(adapter_path) = adapter else {
        return Ok(super::contract::sdk_not_run());
    };
    let spec = read_json(adapter_path)?;
    let adapter: Adapter = serde_json::from_value(spec)?;
    if adapter.schema_revision != 1 || adapter.arguments.len() > 64 || adapter.files.len() > 128 {
        return Err(failure("Unsupported external SDK adapter specification"));
    }
    verify(&adapter.program)?;
    for file in &adapter.files {
        verify(file)?;
    }
    for argument in &adapter.arguments {
        let path = Path::new(argument);
        if !path.is_absolute() || !adapter.files.iter().any(|file| file.path == path) {
            return Err(failure(
                "External SDK adapter arguments must name explicitly pinned absolute files",
            ));
        }
    }
    let spec_digest = digest(adapter_path)?;
    let output = output.join("automation-clients");
    std::fs::create_dir(&output)?;
    let profile = scratch.join("automation-clients-profile");
    if profile.exists() {
        return Err(failure("External SDK adapter profile already exists"));
    }
    let report_path = output.join("clients.json");
    let server = Driver::start(driver, &output.join("chromedriver.log"), environment)?;
    let address = format!("127.0.0.1:{}", server.port());
    super::contract::loopback_port(&json!(address))?;
    let mut command = Command::new(&adapter.program.path);
    // These five arguments preserve the external harness contract. No shell is used.
    command
        .env_clear()
        .envs(environment)
        .args(&adapter.arguments)
        .arg(&address)
        .arg(binary)
        .arg(&profile)
        .arg(fixture_url()?)
        .arg(&report_path)
        .current_dir(
            adapter
                .program
                .path
                .parent()
                .ok_or_else(|| failure("Missing adapter program directory"))?,
        )
        .stdin(Stdio::null())
        .stdout(
            File::options()
                .write(true)
                .create_new(true)
                .open(output.join("adapter.stdout"))?,
        )
        .stderr(
            File::options()
                .write(true)
                .create_new(true)
                .open(output.join("adapter.stderr"))?,
        );
    if !bounded_child(&mut command, 180)?.success() {
        return Err(failure("External SDK adapter failed"));
    }
    verify(&adapter.program)?;
    for file in &adapter.files {
        verify(file)?;
    }
    if digest(adapter_path)? != spec_digest {
        return Err(failure(
            "External SDK adapter specification changed while running",
        ));
    }
    if !std::fs::symlink_metadata(&report_path)?.is_file() {
        return Err(failure(
            "External SDK adapter report must be a regular file",
        ));
    }
    let mut report = read_json(&report_path)?;
    super::contract::validate_sdk_report(&report)?;
    report["adapter_spec_sha256"] = json!(spec_digest);
    report["adapter_program_sha256"] = json!(adapter.program.sha256);
    report["chromedriver_version"] = json!(server.version());
    report["sdk_compatibility_verified"] = json!(true);
    report["evidence_origin"] = json!(
        "Explicit externally supplied SDK adapter; Rust validates its pinned scenario report"
    );
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_pin_refuses_modified_content_and_relative_paths() {
        let temporary = tempfile::tempdir().expect("owned adapter fixture");
        let path = temporary.path().join("external runner مسیر");
        std::fs::write(&path, b"first adapter").expect("write adapter fixture");
        let file = PinnedFile {
            sha256: digest(&path).expect("hash adapter fixture"),
            path,
        };
        assert!(verify(&file).is_ok());
        std::fs::write(&file.path, b"other adapter").expect("change adapter fixture");
        assert!(verify(&file).is_err());
        assert!(
            verify(&PinnedFile {
                path: "relative-adapter".into(),
                sha256: file.sha256
            })
            .is_err()
        );
    }
}
