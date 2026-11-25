use serde::Serialize;
use serde_json::Value;
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    process::{Command, ExitCode, Stdio},
    time::{Duration, Instant},
};
use url::Url;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const REFERENCES: &[(&str, &str)] = &[
    ("R1", "microsoft/playwright"),
    ("R2", "Kaliiiiiiiiii-Vinyzu/patchright"),
    ("R3", "daijro/camoufox"),
    ("R4", "CloakHQ/cloakbrowser"),
    ("R5", "clearcotelabs/clearcote-browser"),
    ("R6", "mattsse/chromiumoxide"),
    ("R7", "vercel-labs/agent-browser"),
    ("R8a", "chromiumembedded/cef"),
    ("R8b", "tauri-apps/cef-rs"),
    ("R9", "ungoogled-software/ungoogled-chromium"),
    ("R10", "apify/fingerprint-suite"),
    ("R11", "daijro/browserforge"),
    ("R12", "brave/adblock-rust"),
    ("R13", "rebrowser/rebrowser-patches"),
    ("R14", "web-platform-tests/wpt"),
    ("R15", "awslabs/tough"),
    ("R16", "servo/servo"),
];
const RATIONALE: &str = "Public architecture/protocol/build reference; no code or patches adopted. Repository license metadata is not a redistribution audit.";

#[derive(Serialize)]
#[serde(untagged)]
enum Reference {
    Unavailable {
        id: &'static str,
        repository: &'static str,
        decision: &'static str,
        status: &'static str,
        copied_code: bool,
    },
    Observed {
        id: &'static str,
        repository: String,
        url: String,
        revision: Option<String>,
        archived: bool,
        last_pushed_at: Option<String>,
        declared_license: Option<String>,
        decision: &'static str,
        copied_code: bool,
        rationale: &'static str,
    },
}

#[derive(Serialize)]
struct Report {
    observed_at: String,
    references: Vec<Reference>,
}

fn main() -> ExitCode {
    match execute() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn execute() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        None => {}
        Some("-h" | "--help") => {
            println!("Usage: numera-research");
            return Ok(());
        }
        Some(_) => return Err("Usage: numera-research".into()),
    }
    let references = std::thread::scope(|scope| {
        let jobs: Vec<_> = REFERENCES
            .chunks(REFERENCES.len().div_ceil(4))
            .map(|group| {
                scope.spawn(move || {
                    group
                        .iter()
                        .map(|&(id, repo)| inspect(id, repo, &api))
                        .collect::<Result<Vec<_>>>()
                })
            })
            .collect();
        let mut observations = Vec::with_capacity(REFERENCES.len());
        for job in jobs {
            observations.extend(job.join().map_err(|_| "Reference worker failed")??);
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(observations)
    })?;
    let count = references.len();
    let report = Report {
        observed_at: format!("{:.6}", jiff::Timestamp::now()).replace('Z', "+00:00"),
        references,
    };
    let out = Path::new("docs/research/references.json");
    let parent = out.parent().ok_or("Missing output directory")?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut file, &report)?;
    writeln!(file)?;
    file.persist(out)?;
    let timestamp = format!("{:.6}", jiff::Timestamp::now()).replace('Z', "+00:00");
    eprintln!(
        "{{\"timestamp\": \"{timestamp}\", \"severity\": \"INFO\", \"component\": \"repository-tool\", \"message\": \"Recorded {count} public references; none adopted\"}}"
    );
    Ok(())
}

fn inspect(
    id: &'static str,
    repo: &'static str,
    fetch: &impl Fn(&str) -> Result<Option<Value>>,
) -> Result<Reference> {
    let repository_endpoint = format!("repos/{repo}");
    let Some(metadata) = fetch(&repository_endpoint)? else {
        return Ok(Reference::Unavailable {
            id,
            repository: repo,
            decision: "reference-only",
            status: "unavailable",
            copied_code: false,
        });
    };
    let branch = required_string(&metadata, "default_branch")?;
    let mut endpoint = Url::parse(&format!(
        "https://api.github.com/{repository_endpoint}/commits/"
    ))?;
    endpoint
        .path_segments_mut()
        .map_err(|()| "Invalid GitHub endpoint")?
        .pop_if_empty()
        .push(&branch);
    let revision = fetch(endpoint.path().trim_start_matches('/'))?
        .map(|row| required_string(&row, "sha"))
        .transpose()?;
    Ok(Reference::Observed {
        id,
        repository: required_string(&metadata, "full_name")?,
        url: required_string(&metadata, "html_url")?,
        revision,
        archived: metadata["archived"]
            .as_bool()
            .ok_or("Missing reference archived status")?,
        last_pushed_at: serde_json::from_value(metadata["pushed_at"].clone())?,
        declared_license: serde_json::from_value(metadata["license"]["spdx_id"].clone())?,
        decision: "reference-only",
        copied_code: false,
        rationale: RATIONALE,
    })
}

fn required_string(value: &Value, name: &str) -> Result<String> {
    value[name]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("Missing reference field {name}").into())
}

fn api(endpoint: &str) -> Result<Option<Value>> {
    let mut response = tempfile::tempfile()?;
    let mut child = Command::new("gh")
        .args(["api", "--hostname", "github.com", endpoint])
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(Stdio::null())
        .stdout(response.try_clone()?)
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return Ok(None),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return match result {
                    Err(error) => Err(error.into()),
                    _ => Err("GitHub reference metadata request timed out".into()),
                };
            }
        }
    }
    use std::io::{Seek, SeekFrom};
    response.seek(SeekFrom::Start(0))?;
    const LIMIT: u64 = 4 * 1024 * 1024;
    let mut bytes = Vec::new();
    response.take(LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return Err("GitHub reference response exceeded limit".into());
    }
    Ok(Some(serde_json::from_slice(&bytes)?))
}

#[cfg(test)]
mod tests {
    use super::{Reference, inspect};
    use serde_json::json;

    #[test]
    fn records_unavailable_and_reference_only_without_adopting_code() {
        let unavailable = inspect("R1", "a/b", &|_| Ok(None)).unwrap();
        assert!(matches!(
            unavailable,
            Reference::Unavailable {
                copied_code: false,
                ..
            }
        ));
        let available = inspect("R1", "a/b", &|path| Ok(Some(match path {
            "repos/a/b" => json!({
                "default_branch": "release/1", "full_name": "a/B", "html_url": "https://github.com/a/B",
                "archived": false, "pushed_at": null, "license": {"spdx_id": "MIT"},
            }),
            "repos/a/b/commits/release%2F1" => json!({"sha": "fixture"}),
            _ => panic!("unexpected endpoint {path}"),
        }))).unwrap();
        assert!(
            matches!(available, Reference::Observed { revision: Some(revision), copied_code: false, .. } if revision == "fixture")
        );
    }
}
