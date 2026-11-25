use super::{Result, VERSION, process::Captured, require};
use serde_json::Value;
use std::{env, fs::OpenOptions, io::Write};

pub fn safe_cell(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
        .replace('|', "&#124;")
        .replace(['\n', '\r'], " ")
}

pub fn validate_scan(result: &Captured, git_status: &[u8], report: Value) -> Result<Vec<Value>> {
    require(
        matches!(result.status.code(), Some(0 | 2)),
        "Gitleaks failed; no clean result was recorded.",
    )?;
    require(
        result.stdout.trim_ascii().is_empty() && result.stderr.trim_ascii().is_empty(),
        "Gitleaks reported scanner errors; no clean result was recorded.",
    )?;
    require(
        git_status == b"0",
        "The Git-history child did not confirm successful completion; no clean result was recorded.",
    )?;
    let findings = report
        .as_array()
        .ok_or("The scanner report format is invalid.")?;
    require(
        findings.iter().all(Value::is_object),
        "The scanner report format is invalid.",
    )?;
    require(
        findings.iter().all(|finding| {
            ["RuleID", "File", "Commit"]
                .iter()
                .all(|key| finding.get(*key).is_none_or(Value::is_string))
                && finding
                    .get("StartLine")
                    .is_none_or(|line| line.is_u64() || line.is_i64())
        }),
        "The scanner report metadata format is invalid.",
    )?;
    require(
        (result.status.code() == Some(2)) != findings.is_empty(),
        "The scanner result and report disagree.",
    )?;
    Ok(findings.clone())
}

fn cell(value: Option<&Value>) -> String {
    match value {
        None => String::new(),
        Some(Value::String(text)) => safe_cell(text),
        Some(Value::Null) => "None".to_owned(),
        Some(Value::Bool(true)) => "True".to_owned(),
        Some(Value::Bool(false)) => "False".to_owned(),
        Some(value) => safe_cell(&value.to_string()),
    }
}

pub fn render_summary(findings: &[Value], scope: &str) -> String {
    let mut lines = vec!["## Gitleaks".to_owned(), String::new(), format!("Scanner: **{VERSION}**"), String::new(), format!("Scope: **{scope}**"), String::new(),
        "Rules: Pinned built-in Gitleaks rules with standard exclusions and three verified public checksum lines. All fetched commits remain in the scan scope.".to_owned(), String::new()];
    if findings.is_empty() {
        lines.push("No matching secrets were found in the scanned Git history.".to_owned());
    } else {
        lines.extend([format!("**{} potential secrets detected.** Review the locations below; rotate any real exposed credentials.", findings.len()), String::new(),
            "Secret values, source snippets, and commit messages are never included in this summary.".to_owned(), String::new(),
            "| Rule | File | Line | Commit |".to_owned(), "| --- | --- | --- | --- |".to_owned()]);
        for finding in findings.iter().take(200) {
            lines.push(format!(
                "| {} |",
                ["RuleID", "File", "StartLine", "Commit"]
                    .map(|key| format!("<code>{}</code>", cell(finding.get(key))))
                    .join(" | ")
            ));
        }
        if findings.len() > 200 {
            lines.extend([String::new(), format!("Showing 200 of {} findings. Repeat the same version locally for the complete report.", findings.len())]);
        }
    }
    format!("{}\n", lines.join("\n"))
}

pub fn write_summary(findings: &[Value], scope: &str) -> Result<()> {
    let Some(path) = env::var_os("GITHUB_STEP_SUMMARY") else {
        return Ok(());
    };
    OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .and_then(|mut file| file.write_all(render_summary(findings, scope).as_bytes()))
        .map_err(|_| "The redacted scanner summary could not be retained.".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(code: i32) -> std::process::ExitStatus {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(code << 8)
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(code as u32)
        }
    }

    #[test]
    fn a_clean_scanner_exit_alone_cannot_prove_a_clean_scan() {
        let captured = |code, out: &[u8], error: &[u8]| Captured {
            status: status(code),
            stdout: out.to_vec(),
            stderr: error.to_vec(),
        };
        assert!(validate_scan(&captured(0, b"", b""), b"0", serde_json::json!([])).is_ok());
        for (result, git, report) in [
            (
                captured(0, b"", b""),
                b"128".as_slice(),
                serde_json::json!([]),
            ),
            (captured(0, b"", b""), b"".as_slice(), serde_json::json!([])),
            (
                captured(0, b"", b"synthetic private diagnostic"),
                b"0".as_slice(),
                serde_json::json!([]),
            ),
            (
                captured(0, b"unexpected", b""),
                b"0".as_slice(),
                serde_json::json!([]),
            ),
            (
                captured(0, b"", b""),
                b"0".as_slice(),
                serde_json::json!([{}]),
            ),
            (
                captured(2, b"", b""),
                b"0".as_slice(),
                serde_json::json!([]),
            ),
            (
                captured(1, b"", b""),
                b"0".as_slice(),
                serde_json::json!([]),
            ),
            (
                captured(0, b"", b""),
                b"0".as_slice(),
                serde_json::json!({}),
            ),
            (
                captured(2, b"", b""),
                b"0".as_slice(),
                serde_json::json!(["invalid finding"]),
            ),
            (
                captured(2, b"", b""),
                b"0".as_slice(),
                serde_json::json!([{"File":{"Secret":"SYNTHETIC_PRIVATE_CANARY"}}]),
            ),
        ] {
            assert!(validate_scan(&result, git, report).is_err());
        }
        assert!(validate_scan(&captured(2, b"", b""), b"0", serde_json::json!([{}])).is_ok());
    }

    #[test]
    fn summary_escapes_metadata_and_never_includes_private_fields() {
        let synthetic = serde_json::json!({"RuleID":"rule|<script>", "File":"مسیر\n\"file\"", "StartLine":9, "Commit":"abc", "Secret":"SYNTHETIC_PRIVATE_CANARY", "Match":"SYNTHETIC_PRIVATE_CANARY", "Message":"SYNTHETIC_PRIVATE_CANARY"});
        let rendered = render_summary(&[synthetic], "fixed scope");
        assert!(!rendered.contains("SYNTHETIC_PRIVATE_CANARY"));
        assert!(rendered.contains("rule&#124;&lt;script&gt;"));
        assert!(rendered.contains("مسیر &quot;file&quot;"));
        assert!(rendered.contains("<code>9</code>"));
    }
}
