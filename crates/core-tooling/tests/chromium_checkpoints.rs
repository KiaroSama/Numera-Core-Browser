use core_tooling::chromium_build::cache::{
    Delta, Inventory, PROBE_LIMITS, Restore, pack_cache, restore_cache, sha256,
};
use core_tooling::chromium_build::{
    metadata::{Host, compiler_workers, producer_context, reviewed_checkpoint, reviewed_delta},
    report::{read_progress, record_stage},
};
use flate2::read::GzDecoder;
use serde_json::json;
use std::{fs, io, path::Path};

const ORIGINAL_BASE: &[u8] = include_bytes!("fixtures/chromium-cache/original-base.tar.gz");
const ORIGINAL_DELTA: &[u8] = include_bytes!("fixtures/chromium-cache/original-delta.tar.gz");
const BASE_SHA: &str = "9300cac49400b9f49c0ba77ed4fe45abc7602e5a1c2e9f690d98ebfe090fcf18";
const DELTA_SHA: &str = "ee508abfe7d38199773b0403b7ef25484be84e37239aa514f78d7863e7f4e95f";

fn extract_fixture(path: &Path, bytes: &[u8]) -> io::Result<()> {
    io::copy(&mut GzDecoder::new(bytes), &mut fs::File::create_new(path)?)?;
    Ok(())
}

#[test]
fn restores_the_original_python_base_and_delta() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let base_path = root.path().join("base.tar");
    let delta_path = root.path().join("delta.tar");
    extract_fixture(&base_path, ORIGINAL_BASE)?;
    extract_fixture(&delta_path, ORIGINAL_DELTA)?;
    let context = json!({
        "host": "Windows", "run_id": "23", "run_attempt": "1",
        "head": "a".repeat(40), "source_lock_sha256": "b".repeat(64)
    });
    let destination = root.path().join("cache");
    let mut inventory = Inventory::new();
    let mut restore = Restore::new(&context, BASE_SHA, PROBE_LIMITS);
    restore_cache(&destination, &base_path, &restore, &mut inventory)?;
    restore.expected_sha256 = DELTA_SHA;
    restore.base_sha256 = Some(BASE_SHA);
    restore_cache(&destination, &delta_path, &restore, &mut inventory)?;
    restore_cache(&destination, &delta_path, &restore, &mut inventory)?;
    assert_eq!(inventory.len(), 2);
    assert_eq!(
        fs::read(destination.join("a/first"))?,
        b"original base\0\xff"
    );
    assert_eq!(
        fs::read(destination.join("b/second"))?,
        "مرحلهٔ دوم".as_bytes()
    );
    Ok(())
}

#[test]
fn cumulative_writer_preserves_the_base_and_rejects_conflicts_before_extraction()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let source = root.path().join("source");
    fs::create_dir(&source)?;
    let context = json!({"host": "Windows", "run_id": "3"});
    let initial = root.path().join("base.tar");
    fs::write(source.join("base"), b"original")?;
    pack_cache(&source, &initial, &context, None, PROBE_LIMITS, None, None)?;
    let base_sha = sha256(&initial)?;
    let mut inventory = Inventory::new();
    let destination = root.path().join("restored");
    let mut restore = Restore::new(&context, &base_sha, PROBE_LIMITS);
    restore_cache(&destination, &initial, &restore, &mut inventory)?;
    let base_inventory = inventory.clone();
    let first_delta = root.path().join("first-delta.tar");
    let base = Delta {
        base_sha256: &base_sha,
        base_entries: &base_inventory,
    };
    fs::write(source.join("existing"), b"first segment")?;
    pack_cache(
        &source,
        &first_delta,
        &context,
        None,
        PROBE_LIMITS,
        Some(&inventory),
        Some(base),
    )?;
    let first_sha = sha256(&first_delta)?;
    restore.expected_sha256 = &first_sha;
    restore.base_sha256 = Some(&base_sha);
    restore_cache(&destination, &first_delta, &restore, &mut inventory)?;
    fs::write(source.join("additional"), b"must not be written")?;
    fs::write(source.join("existing"), b"changed in another job")?;
    let conflicting = root.path().join("conflicting.tar");
    assert!(
        pack_cache(
            &source,
            &conflicting,
            &context,
            None,
            PROBE_LIMITS,
            Some(&inventory),
            Some(base)
        )
        .is_err()
    );
    assert!(!conflicting.exists());
    pack_cache(
        &source,
        &conflicting,
        &context,
        None,
        PROBE_LIMITS,
        None,
        Some(base),
    )?;
    let conflict_sha = sha256(&conflicting)?;
    restore.expected_sha256 = &conflict_sha;
    let failure = restore_cache(&destination, &conflicting, &restore, &mut inventory).unwrap_err();
    assert!(
        failure
            .to_string()
            .contains("Conflicting compiler cache entries")
    );
    assert!(!destination.join("additional").exists());
    assert_eq!(fs::read(destination.join("existing"))?, b"first segment");
    Ok(())
}

#[test]
fn resolves_the_exact_reviewed_windows_continuation() -> Result<(), Box<dyn std::error::Error>> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evidence");
    let base = reviewed_checkpoint(&directory, "44989521034", 1, Host::Windows)?;
    assert_eq!(
        base.key,
        "chromium-staged-44989521034-1-1-windows-2025-vs2026"
    );
    let delta = reviewed_delta(&directory, "45347037383", 4, &base)?;
    assert_eq!(delta.artifact_id, 14_989_305_756);
    assert_eq!(
        delta.sha256,
        "7e8d74501454909a0b864374fe10e6a784b4f0bfb75532db8b8f9e3f94090164"
    );
    let mut consumer = base.context.clone();
    consumer["commands_sha256"] = json!("different command stream");
    consumer["run_id"] = json!("77");
    let restored = producer_context(&consumer, &delta.context);
    assert_eq!(restored["run_id"], "45347037383");
    assert_eq!(restored["commands_sha256"], "different command stream");
    assert!(reviewed_checkpoint(&directory, "../44989521034", 1, Host::Windows).is_err());
    assert!(reviewed_checkpoint(&directory, "44989521034", 1, Host::Linux).is_err());
    Ok(())
}

#[test]
fn keeps_the_reviewed_budget_and_bounded_binary_progress() -> io::Result<()> {
    let gib = 1024_u64.pow(3);
    assert_eq!(compiler_workers(8, 8 * gib, true), 4);
    assert_eq!(compiler_workers(8, 8 * gib - 1, true), 2);
    assert_eq!(compiler_workers(4, 8 * gib, false), 2);
    let root = tempfile::tempdir()?;
    let log = root.path().join("native.log");
    fs::write(
        &log,
        b"\xff\r[12/300] CXX a.cc\r[13/300] LINK a.dll\n[999/2] invalid",
    )?;
    assert_eq!(
        read_progress(&log),
        Some(json!({"completed":13,"total":300}))
    );
    let mut status =
        json!({"result":"RUNNING","source_build":"NOT_RUN","renderer_fixture":"NOT_RUN"});
    record_stage(&mut status, "compile-link");
    assert_eq!(status["source_build"], "RUNNING");
    status["result"] = json!("BLOCKED");
    record_stage(&mut status, "compile-link");
    assert_eq!(status["source_build"], "BLOCKED");
    assert_eq!(status["renderer_fixture"], "NOT_RUN");
    Ok(())
}
