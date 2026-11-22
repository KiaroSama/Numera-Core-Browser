#![cfg(all(
    feature = "package-store",
    target_arch = "x86_64",
    any(target_os = "linux", target_os = "windows")
))]

mod support;

use core_contracts::{BuildPolicy, EngineFamily, ErrorCode};
use core_engines::{EngineStore, describe_runtime_archive, inspect_manifest};
use serde_json::{Value, json};
use std::fs;
use support::{EXPIRES, Key, repository, repository_with_profile_format};

#[test]
fn signed_update_keeps_a_live_pin_and_rolls_back_the_default() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("trusted-root.json");
    let publisher = Key::new(16);
    fs::write(&root, publisher.root(1, &[&publisher])).unwrap();
    let repo = fixture.path().join("repo");
    let disk = fixture.path().join("Core packages مسیر");
    let store = EngineStore::open(&disk).unwrap();
    assert!(!disk.exists());
    let (first, _) = repository(&repo, &publisher, 1, b"first browser", EXPIRES);
    assert_eq!(store.check(&root, &repo).unwrap().sha256, first);
    assert_eq!(store.stage(&root, &repo).unwrap().sha256, first);
    assert_eq!(store.stage(&root, &repo).unwrap().sha256, first);
    store.activate(&first).unwrap();
    let lease = store
        .resolve(&EngineFamily::Chromium, &BuildPolicy::Approved)
        .unwrap();
    assert_eq!(lease.sha256(), first);
    assert_eq!(fs::read(lease.executable()).unwrap(), b"first browser");

    let (next, _) = repository(&repo, &publisher, 2, b"second browser", EXPIRES);
    assert_eq!(store.install(&root, &repo).unwrap().sha256, next);
    assert_eq!(
        store
            .resolve(&EngineFamily::Chromium, &BuildPolicy::Approved)
            .unwrap()
            .sha256(),
        next
    );
    assert_eq!(fs::read(lease.executable()).unwrap(), b"first browser");
    assert_eq!(
        store
            .resolve(
                &EngineFamily::Chromium,
                &BuildPolicy::Pinned {
                    sha256: first.to_uppercase()
                }
            )
            .unwrap()
            .sha256(),
        first
    );
    assert_eq!(store.rollback().unwrap().sha256, first);
    assert_eq!(
        EngineStore::open(&disk)
            .unwrap()
            .resolve(&EngineFamily::Chromium, &BuildPolicy::Approved)
            .unwrap()
            .sha256(),
        first
    );
    repository(
        &repo,
        &publisher,
        2,
        b"different contents under version two",
        EXPIRES,
    );
    assert_eq!(
        store.check(&root, &repo).unwrap_err().code,
        ErrorCode::UpdateSignatureFailure
    );
    repository(&repo, &publisher, 1, b"first browser", EXPIRES);
    assert_eq!(
        store.stage(&root, &repo).unwrap_err().code,
        ErrorCode::UpdateSignatureFailure
    );
}

#[test]
fn rejects_wrong_signatures_expiration_and_unauthenticated_archives() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("trusted-root.json");
    let publisher = Key::new(16);
    let untrusted = Key::new(32);
    fs::write(&root, publisher.root(1, &[&publisher])).unwrap();
    let repo = fixture.path().join("repo");
    let store = EngineStore::open(&fixture.path().join("store")).unwrap();

    repository(&repo, &untrusted, 1, b"wrong signature", EXPIRES);
    assert_eq!(
        store.stage(&root, &repo).unwrap_err().code,
        ErrorCode::UpdateSignatureFailure
    );
    repository(
        &repo,
        &publisher,
        1,
        b"expired metadata",
        "2000-01-01T00:00:00Z",
    );
    assert_eq!(
        store.stage(&root, &repo).unwrap_err().code,
        ErrorCode::UpdateSignatureFailure
    );
    let (_, archive) = repository(&repo, &publisher, 1, b"unaltered", EXPIRES);
    let archive = repo.join("targets").join(archive);
    let mut altered = fs::read(&archive).unwrap();
    altered[24] ^= 2;
    fs::write(archive, altered).unwrap();
    assert_eq!(
        store.install(&root, &repo).unwrap_err().code,
        ErrorCode::UpdateSignatureFailure
    );
    assert_eq!(store.inventory().unwrap()["installed"], json!([]));
}

#[test]
fn authenticated_inventory_catches_resource_edits_and_unexpected_members() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("trusted-root.json");
    let publisher = Key::new(16);
    fs::write(&root, publisher.root(1, &[&publisher])).unwrap();
    let repo = fixture.path().join("repo");
    let store = EngineStore::open(&fixture.path().join("store")).unwrap();
    let (digest, archive) = repository(&repo, &publisher, 1, b"verified browser", EXPIRES);

    let signed_metadata: Value =
        serde_json::from_slice(&fs::read(repo.join("metadata/1.targets.json")).unwrap()).unwrap();
    let signed_target = signed_metadata["signed"]["targets"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap();
    let manifest = inspect_manifest(
        &serde_json::to_vec(&signed_target["custom"]["numera_engine_manifest"]).unwrap(),
    )
    .unwrap();
    let described =
        describe_runtime_archive(&repo.join("targets").join(archive), &digest, &manifest).unwrap();
    assert_eq!(serde_json::to_value(described).unwrap(), *signed_target);

    store.install(&root, &repo).unwrap();
    let lease = store
        .resolve(&EngineFamily::Chromium, &BuildPolicy::Approved)
        .unwrap();
    let runtime = lease.executable().parent().unwrap();
    let unexpected = runtime.join("unexpected.dll");
    fs::write(&unexpected, b"loader shadow").unwrap();
    assert_eq!(
        store.verify(&digest).unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
    fs::remove_file(unexpected).unwrap();

    let resource = runtime.join("locales/داده.pak");
    let permissions = fs::metadata(&resource).unwrap().permissions();
    fs::remove_file(&resource).unwrap();
    fs::write(&resource, b"tamper").unwrap();
    fs::set_permissions(resource, permissions).unwrap();
    assert_eq!(
        store.verify(&digest).unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        store
            .select(&EngineFamily::Chromium, &BuildPolicy::Approved)
            .unwrap()
            .sha256,
        digest
    );
}

#[test]
fn interrupted_publication_can_reuse_only_a_verified_orphan_directory() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("trusted-root.json");
    let publisher = Key::new(16);
    fs::write(&root, publisher.root(1, &[&publisher])).unwrap();
    let repo = fixture.path().join("repo");
    let disk = fixture.path().join("store");
    let store = EngineStore::open(&disk).unwrap();
    let (digest, _) = repository(&repo, &publisher, 1, b"recoverable runtime", EXPIRES);

    store.check(&root, &repo).unwrap();
    let old_state = fs::read(disk.join("state.json")).unwrap();
    store.stage(&root, &repo).unwrap();
    let receipt = disk.join("packages").join(&digest).join("receipt.json");
    let saved_receipt = fs::read(&receipt).unwrap();
    fs::write(disk.join("state.json"), old_state).unwrap();
    assert_eq!(
        store.verify(&digest).unwrap_err().code,
        ErrorCode::MissingPackage
    );
    let mut forged: Value = serde_json::from_slice(&saved_receipt).unwrap();
    let files = forged["members"].as_object_mut().unwrap();
    files.get_mut("locales/داده.pak").unwrap()["sha256"] = json!("0".repeat(64));
    fs::write(&receipt, serde_json::to_vec(&forged).unwrap()).unwrap();
    assert_eq!(
        store.stage(&root, &repo).unwrap_err().code,
        ErrorCode::UpdateSignatureFailure
    );
    fs::write(&receipt, saved_receipt).unwrap();
    assert_eq!(store.install(&root, &repo).unwrap().sha256, digest);
    assert_eq!(store.verify(&digest).unwrap().sha256, digest);
}

#[test]
fn rotated_root_remains_trusted_across_restart_and_failed_metadata_load() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("trusted-root.json");
    let original = Key::new(21);
    fs::write(&root, original.root(1, &[&original])).unwrap();
    let successor = Key::new(22);
    let repo = fixture.path().join("repo");
    let disk = fixture.path().join("store");
    let store = EngineStore::open(&disk).unwrap();
    let (first, _) = repository(&repo, &original, 9, b"first under earlier key", EXPIRES);
    store.install(&root, &repo).unwrap();
    fs::write(
        repo.join("metadata/2.root.json"),
        successor.root(2, &[&original, &successor]),
    )
    .unwrap();
    assert_eq!(
        store.check(&root, &repo).unwrap_err().code,
        ErrorCode::UpdateSignatureFailure
    );
    fs::remove_file(repo.join("metadata/2.root.json")).unwrap();

    let reopened = EngineStore::open(&disk).unwrap();
    assert_eq!(
        reopened.check(&root, &repo).unwrap_err().code,
        ErrorCode::UpdateSignatureFailure
    );
    let (next, _) = repository(&repo, &successor, 1, b"second under rotated key", EXPIRES);
    assert_eq!(reopened.install(&root, &repo).unwrap().sha256, next);
    assert_eq!(reopened.verify(&first).unwrap().sha256, first);
}

#[test]
fn incompatible_rollback_keeps_both_packages_and_the_current_default() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("trusted-root.json");
    let publisher = Key::new(16);
    fs::write(&root, publisher.root(1, &[&publisher])).unwrap();
    let repo = fixture.path().join("repo");
    let store = EngineStore::open(&fixture.path().join("store")).unwrap();

    let (first, _) = repository(&repo, &publisher, 1, b"first", EXPIRES);
    store.install(&root, &repo).unwrap();
    let (current, _) = repository_with_profile_format(
        &repo,
        &publisher,
        2,
        b"second",
        EXPIRES,
        "fixture-new-data",
    );
    store.install(&root, &repo).unwrap();
    assert_eq!(
        store.rollback().unwrap_err().code,
        ErrorCode::IncompatibleEngineDataFormat
    );
    assert_eq!(
        store
            .select(&EngineFamily::Chromium, &BuildPolicy::Approved)
            .unwrap()
            .sha256,
        current
    );
    assert_eq!(store.verify(&first).unwrap().sha256, first);
}

#[test]
fn active_lease_holds_a_shared_operating_system_lock() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("trusted-root.json");
    let disk = fixture.path().join("store");
    let publisher = Key::new(16);
    fs::write(&root, publisher.root(1, &[&publisher])).unwrap();
    let repo = fixture.path().join("repo");
    let store = EngineStore::open(&disk).unwrap();
    let (digest, _) = repository(&repo, &publisher, 1, b"leased package", EXPIRES);
    store.install(&root, &repo).unwrap();

    let lease = store
        .resolve(&EngineFamily::Chromium, &BuildPolicy::Approved)
        .unwrap();
    let direct = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(disk.join("packages").join(&digest).join("lease.lock"))
        .unwrap();
    assert!(matches!(
        direct.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    drop(lease);
    direct.try_lock().unwrap();
    assert_eq!(
        store
            .resolve(&EngineFamily::Chromium, &BuildPolicy::Approved)
            .unwrap_err()
            .code,
        ErrorCode::OwnershipConflict
    );
}

#[cfg(unix)]
#[test]
fn rejects_metadata_and_runtime_symlinks() {
    use std::os::unix::fs::symlink;
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("trusted-root.json");
    let publisher = Key::new(16);
    fs::write(&root, publisher.root(1, &[&publisher])).unwrap();
    let repo = fixture.path().join("repo");
    let store = EngineStore::open(&fixture.path().join("store")).unwrap();
    let (_, _) = repository(&repo, &publisher, 1, b"browser", EXPIRES);

    let timestamp = repo.join("metadata/timestamp.json");
    let external_timestamp = fixture.path().join("timestamp.json");
    fs::rename(&timestamp, &external_timestamp).unwrap();
    symlink(&external_timestamp, &timestamp).unwrap();
    assert_eq!(
        store.check(&root, &repo).unwrap_err().code,
        ErrorCode::UpdateSignatureFailure
    );
    fs::remove_file(&timestamp).unwrap();
    fs::rename(&external_timestamp, &timestamp).unwrap();
    let digest = store.install(&root, &repo).unwrap().sha256;

    let lease = store
        .resolve(&EngineFamily::Chromium, &BuildPolicy::Approved)
        .unwrap();
    let runtime = lease.executable().parent().unwrap();
    let external = fixture.path().join("same browser");
    fs::rename(lease.executable(), &external).unwrap();
    symlink(external, runtime.join("chrome")).unwrap();
    assert_eq!(
        store.verify(&digest).unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
}
