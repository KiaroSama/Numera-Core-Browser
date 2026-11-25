mod distribution;
mod run;
mod runtime;
#[cfg(test)]
mod tests;

use core_tooling::{
    chromium_build::{
        metadata::Host,
        process,
        report::{self, getenv},
    },
    fixtures::Result,
    sha256_file,
};
use distribution::{
    MANIFEST, Selection, failure, valid_revision, verify_download, verify_subjects,
};
use serde_json::{Value, json};
use std::{env, fs, path::Path, process::ExitCode, time::Duration};

const COMPONENT: &str = "chromium-download-smoke";

fn resolve_run() -> Result<()> {
    let id = getenv("BUNDLE_RUN");
    let host = getenv("BUNDLE_HOST");
    let resolved = run::resolve(&id, &host, |command| {
        Ok(process::stdout(
            command,
            Duration::from_secs(30),
            core_contracts::MAX_FRAME as u64,
        )?)
    })?;
    report::github_output(
        &[
            ("run", &id),
            ("sha", &resolved.revision),
            ("target", resolved.target),
        ],
        true,
    )?;
    Ok(())
}

fn selected_subjects() -> Result<()> {
    let host = match getenv("RUNTIME_TARGET").as_str() {
        "ubuntu-24.04" => Host::Linux,
        "windows-2025-vs2026" => Host::Windows,
        _ => return Err(failure("Invalid native development artifact target")),
    };
    let revision = getenv("GITHUB_SHA");
    let run = getenv("GITHUB_RUN_ID").parse()?;
    verify_subjects(
        Path::new("build/tested-runtime"),
        Path::new("."),
        Selection {
            host,
            revision: &revision,
            run,
        },
    )
}

fn read_linux_distribution() -> Result<Value> {
    let data = fs::read_to_string("/etc/os-release")
        .or_else(|_| fs::read_to_string("/usr/lib/os-release"))?;
    let value = |key: &str| {
        data.lines()
            .rev()
            .find_map(|line| {
                let (name, value) = line.split_once('=')?;
                (name == key).then(|| value.trim_matches(['\'', '"']))
            })
            .filter(|value| !value.is_empty())
            .ok_or_else(|| failure("The Linux distribution identity is incomplete"))
    };
    Ok(json!({"id":value("ID")?, "version":value("VERSION_ID")?}))
}

fn read_windows_distribution() -> Result<Value> {
    let bytes = process::stdout(
        process::command("cmd.exe", &process::build_environment()).args(["/d", "/c", "ver"]),
        Duration::from_secs(5),
        65_536,
    )?;
    let text = String::from_utf8_lossy(&bytes);
    let version = text
        .trim()
        .strip_suffix(']')
        .and_then(|line| line.rsplit_once(' '))
        .map(|(_, version)| version)
        .ok_or_else(|| failure("The Windows distribution identity is incomplete"))?;
    let components: Vec<_> = version.split('.').take(3).collect();
    if components.len() != 3
        || components
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(failure("The Windows distribution identity is incomplete"));
    }
    Ok(json!({"id":"windows", "version":components.join(".")}))
}

fn retained_download() -> Result<()> {
    let revision = getenv("NUMERA_BUNDLE_SOURCE_SHA");
    let run = getenv("NUMERA_BUNDLE_RUN").parse::<u64>()?;
    let host = report::validate_runner(false)?;
    if getenv("RUNNER_OS") != host.name()
        || env::consts::ARCH != "x86_64"
        || getenv("GITHUB_REF") != "refs/heads/main"
        || !valid_revision(&revision)
        || run == 0
    {
        return Err(failure(
            "The retained browser test requires an authorized public native runner",
        ));
    }
    let package = Path::new("build/downloaded-chromium/package").canonicalize()?;
    let provenance = Path::new("build/downloaded-chromium/provenance").canonicalize()?;
    let output = std::path::absolute("build/chromium-download-smoke")?;
    fs::create_dir(&output)?;
    let distribution = match host {
        Host::Linux => read_linux_distribution()?,
        Host::Windows => read_windows_distribution()?,
    };
    let mut report = json!({"schema_revision":1,"result":"BLOCKED","artifact_source_commit":revision,
        "artifact_run_id":run,"verification_commit":getenv("GITHUB_SHA"),"runner_image":getenv("ImageVersion"),
        "compiled_chromium":false,"distribution":distribution,"trusted_release":false,
        "installation_scope":"Fresh standard native GitHub-hosted runner"});

    let outcome = (|| -> Result<()> {
        let manifest = verify_download(
            &package,
            &provenance,
            Path::new("."),
            Selection {
                host,
                revision: &revision,
                run,
            },
            |command| {
                process::stdout(command, Duration::from_secs(180), 8 * 1024 * 1024)?;
                Ok(())
            },
        )?;
        report["manifest_sha256"] = json!(sha256_file(&package.join(MANIFEST))?);
        report["source_provenance"] = json!("PASS");
        let scratch = tempfile::Builder::new()
            .prefix("numera-retained-browser-")
            .tempdir_in(report::required_path("RUNNER_TEMP")?)?;
        runtime::verify_and_smoke(
            &package,
            &manifest,
            scratch.path(),
            &output,
            &mut report,
            host,
        )?;
        report["result"] = json!("PASS");
        Ok(())
    })();
    report["timestamp"] = json!(report::now());
    report::write_json(&output.join("report.json"), &report)?;
    outcome
}

fn main() -> ExitCode {
    let result = match env::args_os().skip(1).collect::<Vec<_>>().as_slice() {
        [] => retained_download(),
        [mode] if mode == "--verify-subjects" => selected_subjects(),
        [mode] if mode == "--resolve-run" => resolve_run(),
        _ => Err(failure(
            "Use numera-chromium-download-smoke [--verify-subjects|--resolve-run]",
        )),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            report::log("ERROR", COMPONENT, &error.to_string());
            ExitCode::from(2)
        }
    }
}
