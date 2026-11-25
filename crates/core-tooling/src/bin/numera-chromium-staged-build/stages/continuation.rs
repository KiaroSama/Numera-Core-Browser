use super::{Graph, Result, Stage, getenv, outputs, read_json_bounded, report};
use core_tooling::chromium_build::{
    cache::{self, Delta, Inventory, OBJECT_DELTA_LIMITS, PROBE_LIMITS, Restore, SEGMENT_LIMITS},
    metadata::producer_context,
    process::Environment,
    tools::sccache_count,
};
use serde_json::{Value, json};
use std::{fs, path::Path};

#[derive(Default)]
pub struct Entries {
    base: Inventory,
    current: Inventory,
}

pub fn restore(stage: &Stage, context: &Value, evidence: &mut Value) -> Result<Entries> {
    let mut entries = Entries::default();
    if stage.segment == 1 && stage.base.is_none() {
        return Ok(entries);
    }
    evidence["stage"] = json!("restore");
    let supplied_digest = getenv("EXPECTED_CHECKPOINT_SHA");
    let path = stage.transfer.join("checkpoint.tar");
    let (digest, expected) = if let Some(base) = &stage.base {
        evidence["reviewed_checkpoint"] = json!({"sha256":base.sha256,"key":base.key,
            "job_id":base.job_id,"source_commit":base.context["head"]});
        (
            base.sha256.as_str(),
            producer_context(context, &base.context),
        )
    } else {
        (supplied_digest.as_str(), context.clone())
    };
    let mut options = Restore::new(&expected, digest, SEGMENT_LIMITS);
    options.allow_linux_runner_image = true;
    let restored = cache::restore_cache(&stage.entries, &path, &options, &mut entries.current)?;
    evidence["runner_images"] = restored.runner_images.unwrap_or_default();
    fs::remove_file(&path)?;
    if stage.windows_delta {
        entries.base = entries.current.clone();
        if let Some(recovered) = &stage.inherited_delta {
            evidence["reviewed_delta"] = json!({"run_id":recovered.run_id,"job_id":recovered.job_id,
                "artifact_id":recovered.artifact_id,"sha256":recovered.sha256});
            let expected = producer_context(context, &recovered.context);
            restore_delta(
                stage,
                &expected,
                &recovered.sha256,
                "reviewed-delta",
                &mut entries,
                evidence,
            )?;
        } else if stage.segment > 1 {
            restore_delta(
                stage,
                context,
                &supplied_digest,
                "predecessor",
                &mut entries,
                evidence,
            )?;
        }
    }
    Ok(entries)
}

fn restore_delta(
    stage: &Stage,
    context: &Value,
    digest: &str,
    directory: &str,
    entries: &mut Entries,
    evidence: &mut Value,
) -> Result<()> {
    evidence["stage"] = json!("restore-staged-delta");
    let path = stage.transfer.join(directory).join("delta.tar");
    let mut options = Restore::new(context, digest, SEGMENT_LIMITS);
    options.base_sha256 = Some(
        &stage
            .base
            .as_ref()
            .ok_or("Missing reviewed delta base")?
            .sha256,
    );
    let restored = cache::restore_cache(&stage.entries, &path, &options, &mut entries.current)?;
    evidence["restored_delta_sha256"] = json!(digest);
    evidence["restored_delta_entries"] = json!(restored.entries_added);
    fs::remove_file(path)?;
    Ok(())
}

pub fn object_resume(
    stage: &Stage,
    context: &Value,
    directory: &Path,
    targets: &[String],
    entries: &mut Entries,
    environment: &mut Environment,
    evidence: &mut Value,
) -> Result<Option<Value>> {
    if stage.probe_delta != "resume" {
        return Ok(None);
    }
    evidence["stage"] = json!("restore-object-delta");
    let digest = getenv("EXPECTED_DELTA_SHA");
    let mut options = Restore::new(context, &digest, PROBE_LIMITS);
    options.base_sha256 = Some(
        &stage
            .base
            .as_ref()
            .ok_or("Missing reviewed delta base")?
            .sha256,
    );
    cache::restore_cache(
        &stage.entries,
        &directory.join("delta.tar"),
        &options,
        &mut entries.current,
    )?;
    environment.insert("SCCACHE_LOCAL_RW_MODE".into(), "READ_ONLY".into());
    let file = directory.join("object-sha256.json");
    let expected = read_json_bounded(&file, 32_768)?;
    if cache::sha256(&file)? != getenv("EXPECTED_OBJECTS_SHA") {
        return Err("The producing object inventory is missing or differs from its digest".into());
    }
    let fields = expected
        .as_object()
        .ok_or("The producing object inventory is invalid")?;
    if fields.len() != targets.len() || targets.iter().any(|path| !fields.contains_key(path)) {
        return Err("The producing job selected different compiler targets".into());
    }
    Ok(Some(expected))
}

pub struct ObjectProbe<'a> {
    pub directory: &'a Path,
    pub targets: &'a [String],
    pub cache: &'a Value,
    pub expected: Option<Value>,
}

pub fn object_finish(
    stage: &Stage,
    graph: &Graph,
    entries: &Entries,
    probe: ObjectProbe<'_>,
    evidence: &mut Value,
) -> Result<()> {
    let objects = probe
        .targets
        .iter()
        .map(|name| {
            Ok((
                name.clone(),
                cache::sha256(&graph.native_output.join(name))?,
            ))
        })
        .collect::<std::io::Result<Inventory>>()?;
    let objects = json!(objects);
    evidence["object_probe"]["output_sha256"] = objects.clone();
    let writes = probe.cache["local_storage_write"]
        .as_u64()
        .unwrap_or_default();
    if let Some(expected) = probe.expected {
        let stats = &probe.cache["backend_statistics"];
        if expected != objects
            || writes != 0
            || stats["compilations"] != 0
            || stats["requests_not_cacheable"] != 0
            || sccache_count(stats, "cache_misses") != 0
        {
            return Err(
                "The fresh runner did not reproduce the target objects entirely from cache".into(),
            );
        }
        evidence["object_probe"]["restored_delta_sha256"] = json!(getenv("EXPECTED_DELTA_SHA"));
    } else if writes == 0 {
        return Err("The targeted compiler probe did not add compiler outputs".into());
    }
    let (delta_sha, objects_sha) = if stage.probe_delta == "seed" {
        fs::create_dir(probe.directory)?;
        let archive = probe.directory.join("delta.tar");
        let base = stage.base.as_ref().ok_or("Missing reviewed delta base")?;
        let delta = Delta {
            base_sha256: &base.sha256,
            base_entries: &entries.current,
        };
        cache::pack_cache(
            &stage.entries,
            &archive,
            &graph.context,
            None,
            OBJECT_DELTA_LIMITS,
            Some(&entries.current),
            Some(delta),
        )?;
        let objects_path = probe.directory.join("object-sha256.json");
        report::write_json(&objects_path, &objects)?;
        let delta_sha = cache::sha256(&archive)?;
        let objects_sha = cache::sha256(&objects_path)?;
        evidence["object_probe"]["cache_retained"] = json!(true);
        evidence["object_probe"]["delta_sha256"] = json!(delta_sha);
        evidence["object_probe"]["delta_bytes"] = json!(archive.metadata()?.len());
        evidence["object_probe"]["object_inventory_sha256"] = json!(objects_sha);
        (delta_sha, objects_sha)
    } else {
        (String::new(), String::new())
    };
    evidence["result"] = json!("PASS");
    evidence["stage"] = json!("object-probe-complete");
    evidence["source_build"] = json!("PARTIAL");
    outputs("PROBE_PASS", "", &delta_sha, &objects_sha)
}

pub fn checkpoint(
    stage: &Stage,
    context: &Value,
    entries: &Entries,
    values: &Value,
    evidence: &mut Value,
) -> Result<()> {
    if stage.read_only {
        return Err("The read-only replay exhausted its compile budget".into());
    }
    if values["local_storage_write"].as_u64().unwrap_or_default() == 0 {
        return Err("The segment did not add transferable compiler outputs".into());
    }
    let bytes = values["cache_size_kibibyte"]
        .as_u64()
        .unwrap_or_default()
        .saturating_mul(1024);
    if values["cleanups_performed"].as_u64().unwrap_or_default() > 0
        || (!stage.windows_delta && bytes >= 2_500_000_000)
    {
        return Err("The compiler cache did not fit the bounded checkpoint allowance".into());
    }
    evidence["stage"] = json!("checkpoint");
    let (name, delta) = if stage.windows_delta {
        let base = stage.base.as_ref().ok_or("Missing reviewed delta base")?;
        (
            "delta.tar",
            Some(Delta {
                base_sha256: &base.sha256,
                base_entries: &entries.base,
            }),
        )
    } else {
        ("checkpoint.tar", None)
    };
    let archive = stage.transfer.join(name);
    cache::pack_cache(
        &stage.entries,
        &archive,
        context,
        None,
        SEGMENT_LIMITS,
        Some(&entries.current),
        delta,
    )?;
    let digest = cache::sha256(&archive)?;
    evidence["cache_archive_bytes"] = json!(archive.metadata()?.len());
    evidence["cache_archive_sha256"] = json!(digest);
    evidence["result"] = json!("PARTIAL");
    evidence["source_build"] = json!("PARTIAL");
    outputs("PARTIAL", &digest, "", "")
}
