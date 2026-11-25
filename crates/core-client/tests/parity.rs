use core_client::{ClientError, CoreClient, PlanStatus, TransportError};
use core_contracts::{BuildPolicy, ErrorCode, LaunchSnapshot, MAX_DEADLINE_MS};
use std::{
    env, fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

fn snapshot(root: &Path) -> LaunchSnapshot {
    let mut snapshot: LaunchSnapshot = serde_json::from_str(include_str!(
        "../../../contracts/fixtures/launch-desktop.json"
    ))
    .unwrap();
    snapshot.resources.data_root = root.to_str().unwrap().to_owned();
    snapshot.resources.native_profile_dir = root.join("profile").to_str().unwrap().to_owned();
    snapshot.resources.cache_root = root.join("cache").to_str().unwrap().to_owned();
    snapshot.build_policy = BuildPolicy::Pinned {
        sha256: "a".repeat(64),
    };
    snapshot
}

fn peer(scenario: &str, work: &Path) -> Result<CoreClient, ClientError> {
    CoreClient::connect(
        Path::new(env!("CARGO_BIN_EXE_numera-client-peer-fixture")),
        &work.join(scenario),
        work,
        MAX_DEADLINE_MS,
    )
}

#[test]
#[ignore = "requires NUMERA_CORE_TEST_BINARY pointing to the built Core executable"]
fn test_configured_core_negotiates_and_keeps_empty_store_unmodified() {
    let binary = PathBuf::from(env::var_os("NUMERA_CORE_TEST_BINARY").expect("Core executable"));
    let work = tempfile::Builder::new()
        .prefix("numera-é-")
        .tempdir()
        .unwrap();
    let root = work.path().canonicalize().unwrap();
    let store = root.join("store");
    let data = root.join("data");
    fs::create_dir(&data).unwrap();
    let desired = snapshot(&data);
    let mut changed = desired.clone();
    changed.launch_id = "changed-after-snapshot".into();
    let mut core = CoreClient::connect(&binary, &store, &data, MAX_DEADLINE_MS).unwrap();
    assert_eq!(core.capabilities().namespace, "numera.core");
    assert!(core.list(MAX_DEADLINE_MS).unwrap().is_empty());
    let plan = core.plan(&desired, MAX_DEADLINE_MS).unwrap();
    assert_eq!(plan.launch_id, "fixture-launch");
    assert!(matches!(plan.status, PlanStatus::Blocked));
    assert!(matches!(
        core.launch(&desired, MAX_DEADLINE_MS),
        Err(ClientError::Request(ErrorCode::MissingPackage))
    ));
    assert!(matches!(
        core.status("fixture-launch", MAX_DEADLINE_MS),
        Err(ClientError::Request(ErrorCode::InvalidConfiguration))
    ));
    assert!(core.list(MAX_DEADLINE_MS).unwrap().is_empty());
    assert!(core.close().unwrap().success());
    assert!(core.close().unwrap().success());
    assert!(!store.exists());
    assert!(fs::read_dir(&data).unwrap().next().is_none());
}

#[test]
fn test_wrong_response_id_closes_inherited_stdin() {
    let work = tempfile::tempdir().unwrap();
    assert!(matches!(
        peer("wrong-response-id", work.path()),
        Err(ClientError::Transport(TransportError::MismatchedResponse))
    ));
    let marker = work.path().join("eof");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !marker.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.is_file(), "peer did not observe EOF");
}

#[test]
fn test_request_deadline_includes_a_full_stdin_pipe() {
    let work = tempfile::tempdir().unwrap();
    let mut core = peer("stdin-backpressure", work.path()).unwrap();
    let mut bad_snapshot = snapshot(work.path());
    bad_snapshot.startup_urls = vec!["x".repeat(900_000)];
    let start = Instant::now();
    assert!(matches!(
        core.plan(&bad_snapshot, 500),
        Err(ClientError::Transport(TransportError::Deadline))
    ));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(matches!(
        core.plan(&bad_snapshot, MAX_DEADLINE_MS),
        Err(ClientError::Transport(TransportError::Closed))
    ));
    fs::write(work.path().join("drain"), b"").unwrap();
    assert!(core.close().unwrap().success());
    let bytes: usize = fs::read_to_string(work.path().join("bytes"))
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        bytes > 0 && bytes < 900_000,
        "peer read {bytes} request bytes"
    );
}

#[test]
fn test_unterminated_frame_cannot_grow_past_limit() {
    let work = tempfile::tempdir().unwrap();
    assert!(matches!(
        peer("unterminated-frame", work.path()),
        Err(ClientError::Transport(TransportError::FrameLimit))
    ));
}

#[test]
fn rejects_duplicate_keys_in_a_nested_response() {
    let work = tempfile::tempdir().unwrap();
    assert!(matches!(
        peer("duplicate-key", work.path()),
        Err(ClientError::Transport(TransportError::InvalidResponse))
    ));
}
