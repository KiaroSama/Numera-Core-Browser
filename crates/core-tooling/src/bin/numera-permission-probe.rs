use core_tooling::chromium_build::{metadata::REPOSITORY, process, report};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, ExitCode},
    time::Duration,
};

#[derive(Debug)]
struct Error {
    kind: &'static str,
    status: Option<u16>,
}

impl Error {
    const fn new(kind: &'static str) -> Self {
        Self { kind, status: None }
    }
}

type Result<T> = std::result::Result<T, Error>;

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> std::io::Result<bool> {
    if report::getenv("GITHUB_REPOSITORY") != REPOSITORY
        || report::getenv("GITHUB_ACTIONS") != "true"
    {
        return Err(std::io::Error::other(
            "Permission proof requires the authorized repository workflow",
        ));
    }
    if report::getenv("GH_TOKEN").is_empty() {
        return Err(std::io::Error::other("Workflow credential is missing"));
    }
    let run = report::getenv("GITHUB_RUN_ID");
    let attempt = report::getenv("GITHUB_RUN_ATTEMPT");
    let source = report::getenv("GITHUB_SHA");
    if ![&run, &attempt].iter().all(|value| {
        (1..=20).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_digit())
    }) || !commit_id(&source)
    {
        return Err(std::io::Error::other("Invalid workflow identity"));
    }
    let output = Path::new("build/permission-check.json");
    fs::create_dir_all("build")?;
    let status = prove(&run, &attempt, &source, api);
    report::write_json(output, &status)?;
    if status["error_type"].is_string() {
        report::log(
            "ERROR",
            "permission-probe",
            "Actions permission proof failed; retained status is authoritative",
        );
    }
    let passed = status["result"] == "PASS";
    report::log(
        "INFO",
        "permission-probe",
        if passed {
            "Actions permission proof: PASS"
        } else {
            "Actions permission proof: BLOCKED"
        },
    );
    Ok(passed)
}

fn prove(
    run: &str,
    attempt: &str,
    source: &str,
    mut request: impl FnMut(&str, &str, Option<&Value>) -> Result<Value>,
) -> Value {
    let branch = format!("automation/permission-probe-{run}-{attempt}");
    let mut status = json!({
        "timestamp": report::now(), "commit": source, "run_id": run, "result": "RUNNING",
        "branch": branch, "pull_request": null, "probe_pr_closed": false,
        "probe_branch_removed": false, "engine_promoted": false,
    });
    let mut created = None;
    let mut pull_number = None;
    let proof = (|| {
        let base = sha(&request("GET", "/git/ref/heads/main", None)?["object"]["sha"])?;
        let base_tree =
            sha(&request("GET", &format!("/git/commits/{base}"), None)?["tree"]["sha"])?;
        let notice = format!(
            "{{\"purpose\": \"Temporary Actions PR permission probe\", \"run_id\": \"{run}\"}}"
        );
        let tree = sha(&request(
            "POST",
            "/git/trees",
            Some(&json!({
                "base_tree": base_tree, "tree": [{
                    "path": "docs/evidence/temporary-permission-probe.json", "mode": "100644",
                    "type": "blob", "content": notice,
                }],
            })),
        )?["sha"])?;
        let commit = sha(&request(
            "POST",
            "/git/commits",
            Some(&json!({
                "message": "test: temporary Actions permission probe", "tree": tree, "parents": [base],
            })),
        )?["sha"])?;
        request(
            "POST",
            "/git/refs",
            Some(&json!({
                "ref": format!("refs/heads/{branch}"), "sha": commit,
            })),
        )?;
        created = Some(commit.clone());
        let pull = request(
            "POST",
            "/pulls",
            Some(&json!({
                "title": format!("test: verify Actions PR creation {run}"),
                "head": branch, "base": "main", "draft": true,
                "body": "Temporary permission proof requested by the repository owner. This probe is closed without merging; its task-owned branch is removed after verification. No engine or source lock is promoted.",
            })),
        )?;
        let number = pull["number"]
            .as_u64()
            .filter(|&number| number > 0)
            .ok_or(Error::new("KeyError"))?;
        let author = pull["user"]["login"]
            .as_str()
            .ok_or(Error::new("KeyError"))?;
        let url = pull["html_url"].as_str().ok_or(Error::new("KeyError"))?;
        status["pull_request"] = json!({"number": number, "url": url, "author": author});
        if pull["head"]["ref"] != branch || pull["head"]["sha"] != commit {
            return Err(Error::new("RuntimeError"));
        }
        // A response for another head never authorizes closing that PR.
        pull_number = Some(number);
        if author != "github-actions[bot]" || pull["draft"] != true {
            return Err(Error::new("RuntimeError"));
        }
        Ok(())
    })();
    status["result"] = json!(if proof.is_ok() { "PASS" } else { "BLOCKED" });
    if let Err(error) = proof {
        status["error_type"] = json!(error.kind);
        if let Some(http_status) = error.status {
            status["http_status"] = json!(http_status);
        }
    }
    let cleanup = (|| {
        if let Some(number) = pull_number {
            let closed = request(
                "PATCH",
                &format!("/pulls/{number}"),
                Some(&json!({"state": "closed"})),
            )?;
            if closed["state"] != "closed" {
                return Err(Error::new("RuntimeError"));
            }
            status["probe_pr_closed"] = json!(true);
        }
        if let Some(commit) = created {
            let ref_path = format!("/git/ref/heads/{branch}");
            let current = sha(&request("GET", &ref_path, None)?["object"]["sha"])?;
            if current != commit {
                return Err(Error::new("RuntimeError"));
            }
            request("DELETE", &format!("/git/refs/heads/{branch}"), None)?;
            status["probe_branch_removed"] = json!(true);
        }
        Ok(())
    })();
    if let Err(error) = cleanup {
        status["cleanup_error_type"] = json!(error.kind);
        status["result"] = json!("BLOCKED");
    }
    status
}

fn commit_id(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha(value: &Value) -> Result<String> {
    value
        .as_str()
        .filter(|value| commit_id(value))
        .map(str::to_owned)
        .ok_or(Error::new("KeyError"))
}

fn api(method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
    // The fixed host and repository keep the workflow credential scoped.
    let mut command = Command::new("gh");
    command
        .args([
            "api",
            "--hostname",
            "github.com",
            "--include",
            "--method",
            method,
        ])
        .args(["--header", "Accept: application/vnd.github+json"])
        .args(["--header", "X-GitHub-Api-Version: 2026-03-10"])
        .args(["--header", "User-Agent: Numera-Core-Browser"])
        .env("GH_PROMPT_DISABLED", "1")
        .env_remove("GH_DEBUG");
    let mut input = None;
    if let Some(body) = body {
        let mut file = tempfile::NamedTempFile::new().map_err(|_| Error::new("OSError"))?;
        serde_json::to_writer(&mut file, body).map_err(|_| Error::new("OSError"))?;
        file.flush().map_err(|_| Error::new("OSError"))?;
        command.arg("--input").arg(file.path());
        input = Some(file);
    }
    command.arg(format!("repos/{REPOSITORY}{path}"));
    let result = process::capture(&mut command, Duration::from_secs(20), 1024 * 1024)
        .map_err(|_| Error::new("URLError"))?;
    drop(input);
    let text = std::str::from_utf8(&result.stdout).map_err(|_| Error::new("JSONDecodeError"))?;
    let http_status = text
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("HTTP/"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or(Error::new("URLError"))?;
    if !(200..300).contains(&http_status) {
        return Err(Error {
            kind: "HTTPError",
            status: Some(http_status),
        });
    }
    if !result.status.success() {
        return Err(Error::new("URLError"));
    }
    if http_status == 204 {
        return Ok(Value::Null);
    }
    let (_, body) = text
        .split_once("\r\n\r\n")
        .or_else(|| text.split_once("\n\n"))
        .ok_or(Error::new("JSONDecodeError"))?;
    serde_json::from_str(body).map_err(|_| Error::new("JSONDecodeError"))
}

#[cfg(test)]
mod tests {
    use super::{Error, prove};
    use serde_json::json;

    #[test]
    fn crosses_only_owned_refs_and_preserves_a_reassigned_head() {
        let base = "a".repeat(40);
        let tree = "b".repeat(40);
        let commit = "c".repeat(40);
        for reassigned in [false, true] {
            let mut deleted = false;
            let branch = "automation/permission-probe-1-2";
            let result = prove("1", "2", &base, |method, path, _| {
                Ok(match (method, path) {
                    ("GET", "/git/ref/heads/main") => json!({"object": {"sha": base}}),
                    ("GET", _) if path.ends_with(&base) => json!({"tree": {"sha": tree}}),
                    ("POST", "/git/trees") => json!({"sha": tree}),
                    ("POST", "/git/commits") => json!({"sha": commit}),
                    ("POST", "/git/refs") => json!({}),
                    ("POST", "/pulls") => {
                        json!({"number": 9, "html_url": "fixture", "user": {"login": "github-actions[bot]"},
                        "head": {"ref": branch, "sha": commit}, "draft": true})
                    }
                    ("PATCH", "/pulls/9") => json!({"state": "closed"}),
                    ("GET", _) if path.ends_with(branch) => {
                        json!({"object": {"sha": if reassigned { &base } else { &commit }}})
                    }
                    ("DELETE", _) if path.ends_with(branch) => {
                        deleted = true;
                        json!(null)
                    }
                    _ => return Err(Error::new("unexpected request")),
                })
            });
            assert_eq!(deleted, !reassigned);
            assert_eq!(result["probe_pr_closed"], true);
            assert_eq!(result["probe_branch_removed"], !reassigned);
            assert_eq!(result["engine_promoted"], false);
            assert_eq!(
                result["result"],
                if reassigned { "BLOCKED" } else { "PASS" }
            );
        }
    }
}
