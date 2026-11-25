#[path = "../maintenance/http.rs"]
mod http;
#[path = "../maintenance/lock.rs"]
mod lock;

use http::{HttpError, request_json};
use lock::{Package, Result, read_lock};
use serde::Serialize;
use serde_json::{Value, json};
use std::{fs, io::Write, path::Path, process::ExitCode, time::Duration};

const SOURCE: &str = "https://api.osv.dev/v1/querybatch";

#[derive(Serialize)]
struct Finding {
    package: String,
    version: String,
    advisories: Vec<String>,
}

#[derive(Serialize)]
struct Report {
    timestamp: String,
    source: &'static str,
    lock_file: String,
    result: &'static str,
    packages_checked: usize,
    findings: Vec<Finding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_type: Option<&'static str>,
}

fn main() -> ExitCode {
    match execute() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn execute() -> Result<bool> {
    let mut lock = String::from("Cargo.lock");
    let mut output = String::from("build/security-audit.json");
    let mut args = std::env::args().skip(1);
    while let Some(option) = args.next() {
        match option.as_str() {
            "-h" | "--help" => {
                println!("Usage: numera-security-audit [--lock-file PATH] [--output PATH]");
                return Ok(true);
            }
            "--lock-file" => lock = args.next().ok_or("Missing --lock-file path")?,
            "--output" => output = args.next().ok_or("Missing --output path")?,
            _ => return Err(format!("Unknown audit option: {option}").into()),
        }
    }
    let packages: Vec<_> = read_lock(Path::new(&lock))?
        .package
        .into_iter()
        .filter(|package| package.source.is_some())
        .collect();
    if packages.iter().any(|package| {
        package
            .source
            .as_deref()
            .is_some_and(|source| source.starts_with("registry+"))
            && package.checksum.is_none()
    }) {
        return Err("A locked registry package is missing its checksum".into());
    }
    let mut report = Report {
        timestamp: format!("{:.6}", jiff::Timestamp::now()).replace('Z', "+00:00"),
        source: SOURCE,
        lock_file: lock,
        result: "BLOCKED",
        packages_checked: packages.len(),
        findings: Vec::new(),
        error_type: None,
    };
    // Git dependencies were included in the original crates.io audit. A
    // missing checksum does not turn a locked external package into local code.
    let data = json!({
        "queries": packages.iter().map(|package| json!({
            "package": {"name": package.name, "ecosystem": "crates.io"},
            "version": package.version,
        })).collect::<Vec<_>>(),
    });
    let findings = request_json(
        SOURCE,
        "Numera-Core-Browser",
        Duration::from_secs(45),
        None,
        Some(&data),
    )
    .map_err(|error| match error {
        HttpError::Json(_) => "JSONDecodeError",
        HttpError::Status(_) => "HTTPError",
        HttpError::Origin | HttpError::Transport => "URLError",
    })
    .and_then(|result| interpret(&packages, &result));
    match findings {
        Ok(findings) => {
            report.result = if findings.is_empty() { "PASS" } else { "FAIL" };
            report.findings = findings;
        }
        Err(error) => report.error_type = Some(error),
    }
    let out = Path::new(&output);
    let parent = out
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut file, &report)?;
    writeln!(file)?;
    file.persist(out)?;
    let severity = if report.result == "PASS" {
        "INFO"
    } else {
        "ERROR"
    };
    println!(
        "{{\"timestamp\": \"{}\", \"severity\": \"{severity}\", \"component\": \"dependency-audit\", \"message\": \"OSV audit {}; {} affected packages\"}}",
        report.timestamp,
        report.result,
        report.findings.len()
    );
    Ok(report.result == "PASS")
}

fn interpret(
    packages: &[Package],
    response: &Value,
) -> std::result::Result<Vec<Finding>, &'static str> {
    let results = response
        .get("results")
        .ok_or("KeyError")?
        .as_array()
        .ok_or("TypeError")?;
    if results.len() != packages.len() {
        return Err("AssertionError");
    }
    let mut findings = Vec::new();
    for (package, result) in packages.iter().zip(results) {
        let result = result.as_object().ok_or("TypeError")?;
        if let Some(advisories) = result.get("vulns") {
            let advisories = advisories.as_array().ok_or("TypeError")?;
            if advisories.is_empty() {
                continue;
            }
            let ids = advisories
                .iter()
                .map(|entry| {
                    entry
                        .get("id")
                        .ok_or("KeyError")?
                        .as_str()
                        .map(str::to_owned)
                        .ok_or("TypeError")
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            findings.push(Finding {
                package: package.name.clone(),
                version: package.version.clone(),
                advisories: ids,
            });
        }
    }
    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::{Package, interpret, json};

    #[test]
    fn requires_a_typed_result_for_every_locked_package() {
        let packages = [Package {
            name: "fixture".into(),
            version: "0.1.0".into(),
            source: None,
            checksum: None,
        }];
        assert!(interpret(&packages, &json!({"results": []})).is_err());
        assert!(interpret(&packages, &json!({"results": [null]})).is_err());
        assert!(interpret(&packages, &json!({"results": [{"vulns": {}}]})).is_err());
        assert!(
            interpret(&packages, &json!({"results": [{}]}))
                .unwrap()
                .is_empty()
        );
        let findings = interpret(
            &packages,
            &json!({"results": [{"vulns": [{"id": "OSV-FIXTURE"}]}]}),
        )
        .unwrap();
        assert_eq!(findings[0].advisories, ["OSV-FIXTURE"]);
    }
}
