use super::*;

#[test]
fn regular_evidence_has_a_hard_read_limit() {
    let temporary = tempfile::tempdir().expect("owned evidence fixture");
    let path = temporary.path().join("evidence مسیر");
    fs::write(&path, b"controlled fixture").expect("write evidence fixture");
    assert_eq!(
        read_bounded(&path, 32).expect("bounded regular file"),
        b"controlled fixture"
    );
    assert!(read_bounded(&path, 4).is_err());
    assert!(read_bounded(temporary.path(), 32).is_err());
}

#[test]
fn unverified_archive_never_creates_a_scanner_executable() {
    let temporary = tempfile::tempdir().expect("owned checksum fixture");
    let archive = temporary.path().join("unverified.tar.gz");
    fs::write(&archive, b"synthetic non-release archive").expect("write checksum fixture");
    let result = scanner(temporary.path(), Some(&archive));
    assert_eq!(
        result.unwrap_err(),
        "The Gitleaks download checksum does not match the pinned release."
    );
    assert!(!temporary.path().join("gitleaks").exists());
}

#[test]
fn proxy_helper() {
    let Some(root) = env::var_os("NUMERA_GITLEAKS_PROXY_TEST_ROOT") else {
        return;
    };
    let option = if env::var_os("NUMERA_GITLEAKS_PROXY_TEST_BAD").is_some() {
        "--definitely-invalid-numera-option"
    } else {
        "--all"
    };
    let args = [
        OsString::from("-C"),
        root,
        OsString::from("log"),
        OsString::from(option),
    ];
    std::process::exit(i32::from(process::git_proxy(&args)));
}

#[test]
fn real_git_history_status_is_recorded_and_failure_stays_failure() {
    let temporary = tempfile::tempdir().expect("owned Git proxy fixture");
    let git = process::find_git().expect("native Git");
    let root = temporary.path();
    assert!(
        process::capture(
            process::command(&git)
                .arg("-C")
                .arg(root)
                .args(["init", "--quiet"]),
            15,
            4096
        )
        .expect("initialize owned Git fixture")
        .status
        .success()
    );
    let status = root.join("git-status");
    for expected in [0, 128] {
        let mut command = process::command(env::current_exe().expect("native test helper"));
        command
            .args(["--exact", "tests::proxy_helper", "--nocapture"])
            .env("NUMERA_GITLEAKS_PROXY_TEST_ROOT", root)
            .env("GITLEAKS_REAL_GIT", &git)
            .env("GITLEAKS_GIT_STATUS", &status)
            .env_remove("NUMERA_GITLEAKS_PROXY_TEST_BAD");
        if expected != 0 {
            command.env("NUMERA_GITLEAKS_PROXY_TEST_BAD", "1");
        }
        let result =
            process::capture(&mut command, 5, 64 * 1024).expect("bounded actual Git proxy helper");
        assert_eq!(result.status.code(), Some(expected));
        assert_eq!(
            fs::read(&status).expect("actual Git status"),
            expected.to_string().as_bytes()
        );
        if expected != 0 {
            assert!(
                String::from_utf8_lossy(&result.stderr)
                    .contains("Git-history child did not complete successfully")
            );
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn fifo_helper() {
    let Some(path) = env::var_os("NUMERA_GITLEAKS_FIFO_TEST") else {
        return;
    };
    assert!(read_bounded(Path::new(&path), 32).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn nonregular_fifo_is_rejected_without_blocking() {
    use rustix::fs::{CWD, FileType, Mode, mknodat};
    let temporary = tempfile::tempdir().expect("owned FIFO fixture");
    let path = temporary.path().join("archive-fifo");
    mknodat(CWD, &path, FileType::Fifo, Mode::RUSR | Mode::WUSR, 0).expect("owned FIFO");
    let mut command = process::command(env::current_exe().expect("native test helper"));
    command
        .args(["--exact", "tests::fifo_helper", "--nocapture"])
        .env("NUMERA_GITLEAKS_FIFO_TEST", &path);
    let result = process::capture(&mut command, 2, 4096)
        .expect("FIFO helper must finish before its deadline");
    assert!(result.status.success());
}
