//! Evidence and target contracts shared by the native Chromium build commands.
use super::cache::valid_sha256;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, fs, io, path::Path};

pub const REPOSITORY: &str = "KiaroSama/Numera-Core-Browser";
pub const SEGMENTS: u32 = 4;
const PRODUCER_FIELDS: [&str; 3] = ["head", "run_id", "run_attempt"];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Host {
    Linux,
    Windows,
}

impl Host {
    pub fn current() -> io::Result<Self> {
        match std::env::consts::OS {
            "linux" => Ok(Self::Linux),
            "windows" => Ok(Self::Windows),
            _ => Err(io::Error::other(
                "Chromium tooling requires a Linux or Windows runner",
            )),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Linux => "Linux",
            Self::Windows => "Windows",
        }
    }

    pub fn runner(self) -> &'static str {
        match self {
            Self::Linux => "ubuntu-24.04",
            Self::Windows => "windows-2025-vs2026",
        }
    }

    pub fn targets(self) -> Vec<String> {
        let mut targets = vec!["chrome".into(), "chromedriver".into()];
        if self == Self::Linux {
            targets.push("chrome_sandbox".into());
        }
        targets
    }
}

#[derive(Debug, Serialize)]
pub struct ReviewedBase {
    pub context: Value,
    pub sha256: String,
    pub key: String,
    pub job_id: u64,
}

#[derive(Debug, Serialize)]
pub struct ReviewedDelta {
    pub context: Value,
    pub sha256: String,
    pub artifact_id: u64,
    pub job_id: u64,
    pub run_id: String,
}

struct Checkpoint {
    evidence: Value,
    context: Value,
    sha256: String,
    job_id: u64,
}

fn positive_decimal(value: &str) -> bool {
    !value.is_empty() && !value.starts_with('0') && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn checkpoint_evidence(
    directory: &Path,
    run: &str,
    segment: u32,
    host: Host,
) -> io::Result<Checkpoint> {
    let invalid = || {
        io::Error::other("Reviewed checkpoint metadata does not match the requested native graph")
    };
    if !positive_decimal(run) || run.len() > 18 || !(1..=SEGMENTS).contains(&segment) {
        return Err(io::Error::other(
            "Select a reviewed run ID and checkpoint segment from 1 to 4",
        ));
    }
    let file = directory.join(format!("chromium-staged-{run}-segment-{segment}.json"));
    let missing =
        |_| io::Error::other("No complete reviewed checkpoint evidence exists for this runner");
    let contents = fs::read(&file).map_err(missing)?;
    let evidence: Value = serde_json::from_slice(&contents).map_err(|_| {
        io::Error::other("No complete reviewed checkpoint evidence exists for this runner")
    })?;
    let report = &evidence["report"];
    let context = &report["context"];
    let digest = report["cache_archive_sha256"]
        .as_str()
        .filter(|digest| valid_sha256(digest))
        .ok_or_else(invalid)?;
    let job_id = evidence["job_id"]
        .as_u64()
        .filter(|number| *number > 0)
        .ok_or_else(invalid)?;
    let head = context["head"]
        .as_str()
        .filter(|head| commit(head))
        .ok_or_else(invalid)?;
    if evidence["run_id"].as_u64() != run.parse().ok()
        || report["segment"].as_u64() != Some(u64::from(segment))
        || report["host"].as_str() != Some(host.name())
        || context["host"].as_str() != Some(host.name())
        || context["run_id"].as_str() != Some(run)
        || !context["run_attempt"]
            .as_str()
            .is_some_and(positive_decimal)
        || report["commit"].as_str() != Some(head)
        || report["result"].as_str() != Some("PARTIAL")
        || report["source_build"].as_str() != Some("PARTIAL")
    {
        return Err(invalid());
    }
    let context = context.clone();
    let sha256 = digest.to_owned();
    Ok(Checkpoint {
        evidence,
        context,
        sha256,
        job_id,
    })
}

pub fn reviewed_checkpoint(
    directory: &Path,
    run: &str,
    segment: u32,
    host: Host,
) -> io::Result<ReviewedBase> {
    let Checkpoint {
        evidence,
        context,
        sha256,
        job_id,
    } = checkpoint_evidence(directory, run, segment, host)?;
    let attempt = context["run_attempt"].as_str().expect("validated attempt");
    let key = format!(
        "chromium-staged-{run}-{attempt}-{segment}-{}",
        host.runner()
    );
    if evidence["cache"]["exact_key"].as_str() != Some(&key) {
        return Err(io::Error::other(
            "Reviewed checkpoint has an unexpected cache key",
        ));
    }
    Ok(ReviewedBase {
        context,
        sha256,
        key,
        job_id,
    })
}

pub fn reviewed_delta(
    directory: &Path,
    run: &str,
    segment: u32,
    base: &ReviewedBase,
) -> io::Result<ReviewedDelta> {
    let Checkpoint {
        evidence,
        context,
        sha256,
        job_id,
    } = checkpoint_evidence(directory, run, segment, Host::Windows)?;
    let invalid =
        || io::Error::other("Reviewed delta artifact does not match the reviewed Windows base");
    let report = &evidence["report"];
    let artifact = &evidence["artifact"];
    let previous = &report["reviewed_checkpoint"];
    let attempt = context["run_attempt"].as_str().expect("validated attempt");
    let artifact_name = format!("staged-windows-delta-{attempt}-{segment}");
    let artifact_id = artifact["id"]
        .as_u64()
        .filter(|id| (1..10_u64.pow(18)).contains(id))
        .ok_or_else(invalid)?;
    if report["checkpoint_transport"].as_str() != Some("artifact-delta")
        || artifact["name"].as_str() != Some(artifact_name.as_str())
        || previous["key"].as_str() != Some(base.key.as_str())
        || previous["sha256"].as_str() != Some(base.sha256.as_str())
        || context["source_lock_sha256"] != base.context["source_lock_sha256"]
    {
        return Err(invalid());
    }
    Ok(ReviewedDelta {
        context,
        sha256,
        artifact_id,
        job_id,
        run_id: run.into(),
    })
}

pub fn producer_context(consumer: &Value, producer: &Value) -> Value {
    let mut context = consumer.clone();
    for key in PRODUCER_FIELDS {
        context[key] = producer[key].clone();
    }
    context
}

pub fn windows_object_probe_targets(graph: &Value) -> io::Result<Vec<String>> {
    const CONTROL: &str = "obj/buildtools/third_party/libc++/libc++/algorithm.obj";
    let invalid =
        || io::Error::other("The native graph does not contain the expected probe objects");
    let mut has_control = false;
    let mut outputs = BTreeSet::new();
    for edge in graph.as_array().ok_or_else(invalid)? {
        let name = edge["output"]
            .as_str()
            .ok_or_else(invalid)?
            .replace('\\', "/");
        has_control |= name == CONTROL;
        if name.starts_with("obj/content/browser/")
            && name.ends_with(".obj")
            && name.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'/' | b'-')
            })
            && name.split('/').all(|part| !matches!(part, "" | "." | ".."))
        {
            outputs.insert(name);
        }
    }
    let mut selected = vec![CONTROL.to_owned()];
    selected.extend(outputs.into_iter().take(64));
    if !has_control
        || selected.len() != 65
        || selected.iter().map(|name| name.len() + 1).sum::<usize>() - 1 > 24_000
    {
        return Err(invalid());
    }
    Ok(selected)
}

pub fn compiler_workers(cpus: usize, available: u64, windows_delta: bool) -> usize {
    let gib = 1024_u64.pow(3);
    let reserve = if windows_delta && available >= 8 * gib {
        2 * gib
    } else {
        3 * gib
    };
    cpus.min(4).min((available / reserve) as usize).max(1)
}
