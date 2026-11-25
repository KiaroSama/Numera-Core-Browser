use core_tooling::chromium_build::process::{self, ChildWatch};
use core_tooling::chromium_build::report;
use serde_json::{Value, json};
use std::{
    env, error::Error, fs, os::unix::fs::MetadataExt, path::Path, process::Command, time::Duration,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const REPOSITORY: &str = "KiaroSama/Numera-Core-Browser";
const SDK: &str = "/usr/local/lib/android/sdk";
const MIN_SPACE: u64 = 100_000_000_000;

fn decide(allowed: bool, free: u64, sdk_valid: impl FnOnce() -> Result<bool>) -> Result<bool> {
    if !allowed {
        return Err("Preparation is restricted to the authorized public hosted job".into());
    }
    if free >= MIN_SPACE {
        return Ok(false);
    }
    if !sdk_valid()? {
        return Err("Known unused preinstalled Android SDK could not be verified".into());
    }
    Ok(true)
}

fn verify_sdk() -> Result<bool> {
    let sdk = Path::new(SDK);
    if env::var_os("ANDROID_HOME").as_deref() != Some(sdk.as_os_str()) {
        return Ok(false);
    }
    let metadata = sdk.symlink_metadata()?;
    let mounts = fs::read_to_string("/proc/self/mountinfo")?;
    Ok(metadata.is_dir()
        && sdk.canonicalize()? == sdk
        && !mounts
            .lines()
            .any(|line| line.split_whitespace().nth(4) == Some(SDK))
        && metadata.dev() == Path::new(".").metadata()?.dev()
        && sdk.join("cmdline-tools").is_dir()
        && sdk.join("licenses").is_dir())
}

pub fn run() -> Result<()> {
    let output = Path::new("build/engine-evidence");
    fs::create_dir_all(output)?;
    let free_before = process::disk(Path::new("."))?.free;
    let mut evidence = json!({
        "schema_revision": 1, "run_id": env::var("GITHUB_RUN_ID")?,
        "source_commit": env::var("GITHUB_SHA")?, "result": "BLOCKED",
        "free_before_bytes": free_before, "removed_path": null, "engine_acceptance": "NOT_RUN",
    });
    let outcome = (|| {
        let event: Value =
            serde_json::from_slice(&fs::read(report::required_path("GITHUB_EVENT_PATH")?)?)?;
        let authorized = env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
            && env::var("RUNNER_ENVIRONMENT").as_deref() == Ok("github-hosted")
            && env::var("GITHUB_REPOSITORY").as_deref() == Ok(REPOSITORY)
            && event["repository"]["private"] == false;
        if decide(authorized, free_before, verify_sdk)? {
            evidence["attempted_removal_path"] = json!(SDK);
            let status = ChildWatch::spawn(Command::new("sudo").args([
                "-n",
                "--",
                "rm",
                "-rf",
                "--one-file-system",
                "--",
                SDK,
            ]))?
            .wait(Duration::from_secs(240))?;
            if !status.success() {
                return Err("Bounded removal of the unused Android SDK failed".into());
            }
            evidence["removed_path"] = json!(SDK);
        }
        if process::disk(Path::new("."))?.free < MIN_SPACE {
            return Err("100 GB source preflight remains unmet after bounded preparation".into());
        }
        evidence["result"] = json!("PASS");
        Ok(())
    })();
    evidence["free_after_bytes"] = json!(process::disk(Path::new("."))?.free);
    evidence["timestamp"] = json!(report::now());
    evidence["severity"] = json!(if outcome.is_ok() { "INFO" } else { "ERROR" });
    evidence["component"] = json!("ephemeral-linux-builder");
    report::write_json(&output.join("linux-preparation.json"), &evidence)?;
    println!("{evidence}");
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enough_disk_never_touches_optional_sdk() {
        assert!(
            !decide(true, 120_000_000_000, || panic!(
                "SDK inspection is unnecessary"
            ))
            .unwrap()
        );
    }

    #[test]
    fn unauthorized_job_never_inspects_sdk() {
        assert!(
            decide(false, 91_000_000_000, || panic!(
                "SDK inspection is forbidden"
            ))
            .is_err()
        );
    }

    #[test]
    fn changed_sdk_path_stops_before_removal() {
        assert!(decide(true, 91_000_000_000, || Ok(false)).is_err());
    }
}
