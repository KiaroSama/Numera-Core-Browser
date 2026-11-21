#![cfg(target_os = "linux")]
use std::{
    fs::{self, OpenOptions, TryLockError},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct TestDirectory(PathBuf);
impl TestDirectory {
    fn new() -> Self {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).unwrap();
        let nonce: String = bytes.iter().map(|value| format!("{value:02x}")).collect();
        let path = std::env::temp_dir().join(format!("Numera Linux فارسی {nonce}"));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::write(self.0.join("finish"), b"cleanup");
        if let Ok(lock) = OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.0.join("alive.lock"))
        {
            let deadline = Instant::now() + Duration::from_secs(1);
            while matches!(lock.try_lock(), Err(TryLockError::WouldBlock))
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct FixtureChild<'a> {
    child: Child,
    root: &'a Path,
}
impl<'a> FixtureChild<'a> {
    fn start(command: &mut Command, root: &'a Path) -> Self {
        let child = command
            .arg(root)
            .stdin(Stdio::null())
            .env_clear()
            .spawn()
            .unwrap();
        Self { child, root }
    }

    fn wait_success(&mut self, message: &str) {
        wait_until(|| self.child.try_wait().unwrap().is_some(), message);
        assert!(self.child.wait().unwrap().success());
    }
}
impl Drop for FixtureChild<'_> {
    fn drop(&mut self) {
        if self.child.try_wait().is_ok_and(|status| status.is_some()) {
            return;
        }
        let _ = fs::write(self.root.join("stop"), b"cleanup");
        let _ = fs::write(self.root.join("finish"), b"cleanup");
        let deadline = Instant::now() + Duration::from_secs(4);
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn<'a>(role: &str, root: &'a Path) -> FixtureChild<'a> {
    FixtureChild::start(
        Command::new(env!("CARGO_BIN_EXE_numera-process-fixture")).arg(role),
        root,
    )
}

fn wait_until(mut ready: impl FnMut() -> bool, message: &str) {
    let limit = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(Instant::now() < limit, "{message}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn stopping_owner_reaps_child_in_another_process_group_only() {
    let owned = TestDirectory::new();
    let other = TestDirectory::new();
    let mut owner = spawn("linux-owner", &owned.0);
    let mut unrelated = spawn("linux-owner", &other.0);
    wait_until(
        || owned.0.join("owner-ready").is_file(),
        "Owner did not observe root exit",
    );
    wait_until(
        || other.0.join("owner-ready").is_file(),
        "Separate session worker did not observe root exit",
    );
    let open_lock = |root: &Path| {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.join("alive.lock"))
            .unwrap()
    };
    let orphan_lock = open_lock(&owned.0);
    let unrelated_lock = open_lock(&other.0);
    assert!(matches!(
        orphan_lock.try_lock(),
        Err(TryLockError::WouldBlock)
    ));
    fs::write(owned.0.join("stop"), b"stop").unwrap();
    owner.wait_success("Owner did not finish stopping");
    let released = orphan_lock.try_lock();
    assert!(matches!(
        unrelated_lock.try_lock(),
        Err(TryLockError::WouldBlock)
    ));
    assert!(unrelated.child.try_wait().unwrap().is_none());
    fs::write(other.0.join("stop"), b"cleanup").unwrap();
    unrelated.wait_success("Separate session worker did not finish stopping");
    assert!(
        released.is_ok(),
        "Stopping the supervisor left its adopted browser descendant running"
    );
}

#[test]
fn worker_with_an_existing_child_refuses_to_start_a_session() {
    let directory = TestDirectory::new();
    let mut worker = spawn("linux-reject-shared", &directory.0);
    worker.wait_success("Shared-worker rejection did not finish");
    assert!(!directory.0.join("argv.json").exists());
}

#[test]
fn worker_with_inherited_ignored_sigchld_refuses_to_start_a_session() {
    let directory = TestDirectory::new();
    let mut worker = FixtureChild::start(
        Command::new("/bin/bash")
            .args(["-c", "trap '' CHLD; exec \"$@\"", "numera-fixture"])
            .arg(env!("CARGO_BIN_EXE_numera-process-fixture"))
            .arg("linux-reject-sigchld"),
        &directory.0,
    );
    worker.wait_success("Inherited signal-disposition check did not finish");
    assert!(!directory.0.join("argv.json").exists());
}
