use super::distribution::{failure, valid_revision};
use core_tooling::{
    chromium_build::metadata::{Host, REPOSITORY},
    fixtures::Result,
};
use std::process::Command;

const WORKFLOWS: [u64; 2] = [426002445, 425565660];

pub(super) struct Resolved {
    pub(super) revision: String,
    pub(super) target: &'static str,
}

pub(super) fn resolve(
    run: &str,
    host: &str,
    read_metadata: impl FnOnce(&mut Command) -> Result<Vec<u8>>,
) -> Result<Resolved> {
    if !(1..=18).contains(&run.len())
        || run.starts_with('0')
        || !run.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(failure("Select a successful numeric repository build run"));
    }
    let target = match host {
        "ubuntu-24.04" | "ubuntu-22.04" => Host::Linux.runner(),
        "windows-2025-vs2026" => Host::Windows.runner(),
        _ => return Err(failure("Invalid native download test target")),
    };
    let uri = format!("repos/{REPOSITORY}/actions/runs/{run}");
    let bytes = read_metadata(Command::new("gh").args(["api", &uri]))?;
    let metadata = core_contracts::strict_json(&bytes)?;
    let error = || failure("The selected build is outside the retained-package verification scope");
    if metadata["workflow_id"]
        .as_u64()
        .is_none_or(|id| !WORKFLOWS.contains(&id))
        || metadata["id"].as_u64() != Some(run.parse()?)
        || metadata["status"] != "completed"
        || metadata["conclusion"] != "success"
        || metadata["event"] != "workflow_dispatch"
        || metadata["head_branch"] != "main"
        || metadata["head_repository"]["full_name"] != REPOSITORY
    {
        return Err(error());
    }
    let revision = metadata["head_sha"]
        .as_str()
        .filter(|sha| valid_revision(sha))
        .ok_or_else(error)?
        .to_owned();
    Ok(Resolved { revision, target })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejected_run_numbers_never_reach_the_run_reader() {
        for run in ["", "0", "001", "HEAD", "1234567890123456789", "17/../18"] {
            assert!(
                resolve(run, "ubuntu-24.04", |_| panic!(
                    "bad run ID reached command"
                ))
                .is_err()
            );
        }
    }

    #[test]
    fn selects_the_native_artifact_only_from_allowed_completed_main_builds() {
        let mut record = json!({"id":17,"workflow_id":426002445,"status":"completed",
            "conclusion":"success","event":"workflow_dispatch","head_branch":"main",
            "head_repository":{"full_name":REPOSITORY}, "head_sha":"a".repeat(40)});
        let winner = resolve("17", "ubuntu-22.04", |command| {
            assert_eq!(command.get_program(), "gh");
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                ["api", "repos/KiaroSama/Numera-Core-Browser/actions/runs/17"]
            );
            Ok(serde_json::to_vec(&record)?)
        })
        .unwrap();
        assert_eq!(winner.target, "ubuntu-24.04");
        assert_eq!(winner.revision, "a".repeat(40));
        record["workflow_id"] = json!(425565660);
        assert_eq!(
            resolve("17", "windows-2025-vs2026", |_| Ok(serde_json::to_vec(
                &record
            )?))
            .unwrap()
            .target,
            "windows-2025-vs2026"
        );
        for (key, invalid) in [
            ("workflow_id", json!(42)),
            ("id", json!(18)),
            ("status", json!("queued")),
            ("conclusion", json!("failure")),
            ("event", json!("push")),
            ("head_branch", json!("fork")),
            ("head_repository", json!({"full_name":"foreign/repository"})),
            ("head_sha", json!("ABCDEF")),
        ] {
            let mut invalid_record = record.clone();
            invalid_record[key] = invalid;
            assert!(
                resolve("17", "ubuntu-22.04", |_| Ok(serde_json::to_vec(
                    &invalid_record
                )?))
                .is_err()
            );
        }
    }
}
