use core_tooling::{sha256_file, sha256_hex};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

#[derive(Deserialize)]
struct Lock {
    patches: Vec<Patch>,
    platform_pins: BTreeMap<String, SourcePin>,
}

#[derive(Deserialize)]
struct Patch {
    path: PathBuf,
    sha256: String,
}

#[derive(Deserialize)]
struct SourcePin {
    branding_source_sha256: String,
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let upstream = match (args.next(), args.next(), args.next()) {
        (Some(flag), Some(upstream), None) if flag == "--upstream-file" => upstream,
        _ => return Err("Usage: numera-check-brand-patch --upstream-file FILE".into()),
    };
    let lock: Lock = serde_json::from_slice(&fs::read("upstream/chromium/lock.json")?)?;
    let patch = lock.patches.first().ok_or("No reviewed branding patch")?;
    let patch_path = fs::canonicalize(&patch.path)?;
    if sha256_file(&patch_path)? != patch.sha256 {
        return Err("Branding patch digest mismatch".into());
    }
    let source = fs::read(upstream)?;
    let source_digest = sha256_hex(&source);
    if !lock
        .platform_pins
        .values()
        .any(|pin| pin.branding_source_sha256 == source_digest)
    {
        return Err("Supplied source file is not the pinned upstream fixture".into());
    }

    let scratch = tempfile::Builder::new().prefix("numera-patch-").tempdir()?;
    let source_dir = scratch.path().join("chrome/app");
    fs::create_dir_all(&source_dir)?;
    let target = source_dir.join("chromium_strings.grd");
    fs::write(&target, source)?;
    for args in [["apply", "--check"].as_slice(), ["apply"].as_slice()] {
        if !Command::new("git")
            .args(args)
            .arg(&patch_path)
            .current_dir(scratch.path())
            .status()?
            .success()
        {
            return Err("Applying the reviewed branding patch failed".into());
        }
    }
    let text = fs::read_to_string(target)?.replace("\r\n", "\n");
    if text.matches("            Numera Core Browser\n").count() != 2
        || !text.contains("Google Chrome for Testing")
    {
        return Err("The patched branding fixture did not match the expected names".into());
    }
    eprintln!(
        "{}",
        json!({
            "timestamp": jiff::Timestamp::now().to_string(),
            "severity": "INFO",
            "component": "repository-tool",
            "message": "PASS: patch applies to supplied pinned source; browser compilation remains NOT_RUN",
        })
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Branding patch check failed: {error}");
            ExitCode::FAILURE
        }
    }
}
