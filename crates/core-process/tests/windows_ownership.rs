#![cfg(windows)]
use core_contracts::ErrorCode;
use core_process::{StandaloneLease, windows::OwnedProcessTree};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, OpenOptions, TryLockError},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).unwrap();
        let nonce: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let path = std::env::temp_dir().join(format!("Numera Windows فارسی {nonce}"));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_numera-process-fixture"))
}
fn environment() -> BTreeMap<String, OsString> {
    std::env::var_os("SystemRoot")
        .map(|v| ("SystemRoot".to_owned(), v))
        .into_iter()
        .collect()
}
fn spawn(role: &str, root: &Root) -> OwnedProcessTree {
    OwnedProcessTree::spawn(
        &fixture(),
        &[role.into(), root.0.as_os_str().to_owned()],
        &root.0,
        &environment(),
    )
    .unwrap()
}
fn until(mut predicate: impl FnMut() -> bool) {
    let start = Instant::now();
    while !predicate() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "Native fixture deadline"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn actual_unicode_executable_argv_and_explicit_environment() {
    if std::env::var_os("GITHUB_ACTIONS").is_some() {
        assert!(std::env::var_os("NUMERA_PARENT_SECRET").is_some());
    }
    let root = Root::new();
    let program = root.0.join("فرایند با فاصله.exe");
    fs::copy(fixture(), &program).unwrap();
    let values = [
        "",
        "two words",
        "literal\"quote",
        "C:\\with space\\",
        "فارسی",
        "\\\"edge\\",
    ];
    let mut args = vec!["echo".into(), root.0.as_os_str().to_owned()];
    args.extend(values.iter().map(OsString::from));
    let mut environment = environment();
    environment.insert("NUMERA_MARKER".into(), "مقدار value".into());
    let tree = OwnedProcessTree::spawn(&program, &args, &root.0, &environment).unwrap();
    assert!(tree.pid() > 0 && tree.birth_ticks() > 0);
    until(|| tree.active_processes().unwrap() == 0);
    let received: Vec<String> =
        serde_json::from_slice(&fs::read(root.0.join("argv.json")).unwrap()).unwrap();
    assert_eq!(received, values);
    let environment: serde_json::Value =
        serde_json::from_slice(&fs::read(root.0.join("environment.json")).unwrap()).unwrap();
    assert_eq!(environment["marker"], "مقدار value");
    assert_eq!(environment["parent_token_present"], false);
}

#[test]
fn parent_exit_does_not_release_descendants_or_stop_another_tree() {
    let root = Root::new();
    let other_root = Root::new();
    let tree = spawn("parent", &root);
    let other = spawn("leaf", &other_root);
    until(|| root.0.join("parent-ready").exists() && other_root.0.join("leaf-ready").exists());
    until(|| tree.root_exited().unwrap());
    // Native accounting can still include a terminating parent or console
    // helper. The invariant is live owned descendants, not an exact count.
    assert!(tree.active_processes().unwrap() > 0);
    let descendant = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.0.join("alive.lock"))
        .unwrap();
    assert!(matches!(
        descendant.try_lock(),
        Err(TryLockError::WouldBlock)
    ));
    tree.terminate_and_wait(Duration::from_secs(5)).unwrap();
    assert_eq!(tree.active_processes().unwrap(), 0);
    assert!(descendant.try_lock().is_ok());
    assert!(!other.root_exited().unwrap());
    assert!(other.active_processes().unwrap() > 0);
    other.terminate_and_wait(Duration::from_secs(5)).unwrap();
}

#[test]
fn breakaway_is_rejected_by_the_atomic_job() {
    let root = Root::new();
    let tree = spawn("breakaway", &root);
    until(|| tree.active_processes().unwrap() == 0);
    assert_eq!(fs::read(root.0.join("breakaway.json")).unwrap(), b"false");
}

#[test]
fn owner_crash_closes_job_and_preserves_recovery_claim() {
    let root = Root::new();
    let mut owner = Command::new(fixture())
        .arg("owner")
        .arg(&root.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !root.0.join("owner-ready").exists() {
        if start.elapsed() > Duration::from_secs(10) {
            let _ = owner.kill();
            let _ = owner.wait();
            panic!("Owner fixture failed to start");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let alive = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.0.join("alive.lock"))
        .unwrap();
    assert!(matches!(alive.try_lock(), Err(TryLockError::WouldBlock)));
    owner.kill().unwrap();
    owner.wait().unwrap();
    // The controlled child holds this exact file lock for its entire lifetime.
    // Its release after killing only the owner proves kill-on-job-close cleanup
    // without signaling or trusting any potentially reused descendant PID.
    until(|| match alive.try_lock() {
        Ok(()) => true,
        Err(TryLockError::WouldBlock) => false,
        Err(e) => panic!("{e}"),
    });
    assert_eq!(
        StandaloneLease::acquire(&root.0, "crash", "retry", "key")
            .err()
            .unwrap()
            .code,
        ErrorCode::RecoveryRequired
    );
}

#[test]
fn non_native_programs_and_case_aliased_environment_are_rejected() {
    let root = Root::new();
    assert!(
        OwnedProcessTree::spawn(Path::new("relative.exe"), &[], &root.0, &environment()).is_err()
    );
    let mut environment = environment();
    environment.insert("Path".into(), "one".into());
    environment.insert("PATH".into(), "two".into());
    assert!(OwnedProcessTree::spawn(&fixture(), &[], &root.0, &environment).is_err());
}
