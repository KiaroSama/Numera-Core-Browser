#![cfg(target_os = "linux")]

#[path = "../../core-engines/tests/support/mod.rs"]
mod signed_fixture;

use core_client::{ClientError, CoreClient, PlanStatus, SessionState};
use core_contracts::{BuildPolicy, ErrorCode, LaunchSnapshot, MAX_DEADLINE_MS};
use std::{
    env, fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::Command,
};

const BUDGET: u32 = MAX_DEADLINE_MS;

fn snapshot(root: &Path, id: &str, digest: &str) -> LaunchSnapshot {
    let mut snapshot: LaunchSnapshot = serde_json::from_str(include_str!(
        "../../../contracts/fixtures/launch-desktop.json"
    ))
    .unwrap();
    snapshot.launch_id = id.into();
    snapshot.idempotency_key = format!("{id}-once");
    snapshot.profile_id = format!("{id}-profile");
    snapshot.resources.data_root = root.to_str().unwrap().to_owned();
    snapshot.resources.native_profile_dir = root
        .join(format!("{id}-profile"))
        .to_str()
        .unwrap()
        .to_owned();
    snapshot.resources.cache_root = root
        .join(format!("{id}-cache"))
        .to_str()
        .unwrap()
        .to_owned();
    snapshot.build_policy = BuildPolicy::Pinned {
        sha256: digest.into(),
    };
    snapshot
}

#[test]
#[ignore = "requires NUMERA_CORE_TEST_BINARY pointing to the built Core executable"]
fn signed_process_lifecycle_and_eof_release() {
    let binary = PathBuf::from(env::var_os("NUMERA_CORE_TEST_BINARY").expect("Core executable"));
    let work = tempfile::tempdir().unwrap();
    let root = work.path().canonicalize().unwrap();
    let store = root.join("store");
    let data = root.join("data");
    fs::DirBuilder::new().mode(0o700).create(&data).unwrap();

    let key = signed_fixture::Key::new(16);
    let trusted_root = root.join("root.json");
    fs::write(&trusted_root, key.root(1, &[&key])).unwrap();
    let repository = root.join("repository");
    let executable = b"#!/bin/sh\nfor arg in \"$@\"; do case \"$arg\" in --user-data-dir=*) data=${arg#*=};; esac; done\nprintf 'fixture profile' > \"$data/preferences\"\nexec /bin/sleep 25\n";
    let (digest, _) =
        signed_fixture::repository(&repository, &key, 1, executable, signed_fixture::EXPIRES);
    let install = Command::new(&binary)
        .args(["engines", "install"])
        .args([&store, &trusted_root, &repository])
        .output()
        .unwrap();
    assert!(
        install.status.success(),
        "signed fixture installation failed"
    );

    let mut client = CoreClient::connect(&binary, &store, &data, BUDGET).unwrap();
    assert_eq!(
        client.capabilities().scope.engine_build.as_deref(),
        Some(digest.as_str())
    );
    let desired = snapshot(&data, "first", &digest);
    assert_eq!(
        client.plan(&desired, BUDGET).unwrap().status,
        PlanStatus::Planned
    );
    assert!(fs::read_dir(&data).unwrap().next().is_none());

    let mut unsupported = desired.clone();
    unsupported.required_capabilities = vec!["session.launch".into()];
    assert!(matches!(
        client.launch(&unsupported, BUDGET),
        Err(ClientError::Request(ErrorCode::UnsupportedCapability))
    ));
    assert!(fs::read_dir(&data).unwrap().next().is_none());
    let launch = client.launch(&desired, BUDGET).unwrap();
    assert_eq!(launch.state, SessionState::Running);
    assert!(launch.browser_process_started);
    assert!(launch.automation_endpoints.is_empty());
    assert_eq!(launch.package_sha256, digest);
    assert!(client.launch(&desired, BUDGET).unwrap() == launch);
    assert!(client.attach("first", BUDGET).unwrap() == launch);
    assert!(client.status("first", BUDGET).unwrap() == launch);
    let initial = client.events("first", 0, BUDGET).unwrap();
    assert!(initial.events == [launch]);
    assert_eq!(initial.next_sequence, 1);
    assert!(
        client
            .events("first", initial.next_sequence, BUDGET)
            .unwrap()
            .events
            .is_empty()
    );

    let second = snapshot(&data, "second", &digest);
    assert_eq!(
        client.launch(&second, BUDGET).unwrap().state,
        SessionState::Running
    );
    assert_eq!(client.list(BUDGET).unwrap().len(), 2);
    let stopped = client.stop("first", BUDGET).unwrap();
    assert_eq!(stopped.state, SessionState::Stopped);
    let terminal = client
        .events("first", initial.next_sequence, BUDGET)
        .unwrap();
    assert!(terminal.events == [stopped]);
    assert_eq!(terminal.next_sequence, 2);
    assert_eq!(
        client.status("second", BUDGET).unwrap().state,
        SessionState::Running
    );
    assert!(client.close().unwrap().success());

    let source = data.join("second-profile");
    let destination = data.join("backup");
    let copy = Command::new(&binary)
        .args(["profiles", "snapshot"])
        .args([&data, &source, &destination])
        .output()
        .unwrap();
    assert!(
        copy.status.success(),
        "profile admission remained held after EOF"
    );
    assert_eq!(
        fs::read(destination.join("preferences")).unwrap(),
        b"fixture profile"
    );
}
