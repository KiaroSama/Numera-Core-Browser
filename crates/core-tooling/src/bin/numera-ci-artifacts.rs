use core_tooling::{
    chromium_build::{process, report},
    sha256_file,
};
use serde_json::Value;
use std::{
    env,
    error::Error,
    fs,
    path::Path,
    process::{Command, ExitCode},
    time::Duration,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn verify_cache_headroom() -> Result<()> {
    report::validate_runner(true)?;
    let mut command = Command::new("gh");
    command.args([
        "api",
        "repos/KiaroSama/Numera-Core-Browser/actions/cache/usage",
    ]);
    let response = process::stdout(&mut command, Duration::from_secs(25), 1024 * 1024)?;
    let response: Value = serde_json::from_slice(&response)?;
    let used = response["active_caches_size_in_bytes"]
        .as_u64()
        .ok_or("Cannot verify repository cache allowance")?;
    let checkpoint = fs::metadata("build/chromium-staged-cache/checkpoint.tar")?.len();
    if used.saturating_add(checkpoint).saturating_add(150_000_000) >= 9_500_000_000 {
        return Err("Repository cache headroom is too small for this checkpoint".into());
    }
    Ok(())
}

fn stage_development_subjects() -> Result<()> {
    let contracts = Path::new("build/attestation-input/contracts");
    let zip_name = "numera-core-contracts-v1.zip";
    let zip = contracts.join(zip_name);
    let sidecar = fs::read_to_string(contracts.join("numera-core-contracts-v1.sha256"))?;
    let expected = format!("{}  {zip_name}", sha256_file(&zip)?);
    if sidecar.trim_end_matches(['\r', '\n']) != expected {
        return Err("The tested contracts ZIP and checksum differ".into());
    }
    let subjects = Path::new("build/attestation-subjects");
    fs::create_dir_all(subjects)?;
    for (input, name) in [
        (
            "development-cli-ubuntu-24.04",
            "numera-core-cli-linux-x86_64.tar.gz",
        ),
        (
            "development-cli-windows-2025",
            "numera-core-cli-windows-x86_64.tar.gz",
        ),
    ] {
        let source = Path::new("build/attestation-input")
            .join(input)
            .join("development-cli.tar.gz");
        if !source.symlink_metadata()?.is_file() {
            return Err("The tested CLI subject must be a regular file".into());
        }
        fs::rename(source, subjects.join(name))?;
    }
    fs::rename(zip, subjects.join(zip_name))?;
    Ok(())
}

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let outcome = match (args.next().as_deref(), args.next()) {
        (Some("verify-cache-headroom"), None) => verify_cache_headroom(),
        (Some("stage-development-subjects"), None) => stage_development_subjects(),
        _ => Err(
            "Usage: numera-ci-artifacts verify-cache-headroom|stage-development-subjects".into(),
        ),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            report::log("ERROR", "ci-artifacts", &error.to_string());
            ExitCode::FAILURE
        }
    }
}
