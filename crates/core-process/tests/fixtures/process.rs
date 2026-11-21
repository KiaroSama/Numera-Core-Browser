//! Development-only executable for actual native process ownership tests.
use std::{
    fs::{self, File},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn child(role: &str, root: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .arg(role)
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.env_remove("GH_TOKEN").env_remove("GITHUB_TOKEN");
    command
}
fn await_file(path: &Path) {
    let start = Instant::now();
    while !path.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "Fixture readiness timeout"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn hold(root: &Path) {
    let file = File::create(root.join("alive.lock")).unwrap();
    file.lock().unwrap();
    fs::write(root.join("leaf-ready"), std::process::id().to_string()).unwrap();
    let start = Instant::now();
    while !root.join("finish").exists() && start.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(any(target_os = "linux", windows))]
fn chromium_owner(root: &Path, browser: &Path, fixture: &std::ffi::OsStr) {
    #[cfg(target_os = "linux")]
    use core_process::linux::OwnedProcessTree;
    #[cfg(windows)]
    use core_process::windows::OwnedProcessTree;
    use std::{collections::BTreeMap, ffi::OsString, io::Read};

    let mut environment = BTreeMap::new();
    for key in [
        "PATH",
        "LANG",
        "SystemRoot",
        "WINDIR",
        "SYSTEMDRIVE",
        "CHROME_DEVEL_SANDBOX",
    ] {
        if let Some(value) = std::env::var_os(key) {
            environment.insert(key.to_owned(), value);
        }
    }
    let home = root.join("home");
    fs::create_dir(&home).unwrap();
    environment.insert("HOME".to_owned(), home.as_os_str().to_owned());
    #[cfg(windows)]
    {
        // Chrome must resolve the Windows default profile to decide whether
        // DevTools is allowed for the explicit isolated profile below.
        for key in [
            "USERPROFILE",
            "USERNAME",
            "HOMEDRIVE",
            "HOMEPATH",
            "LOCALAPPDATA",
            "APPDATA",
            "ALLUSERSPROFILE",
            "ProgramData",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramW6432",
        ] {
            if let Some(value) = std::env::var_os(key) {
                environment.insert(key.to_owned(), value);
            }
        }
        for key in ["TEMP", "TMP"] {
            environment.insert(key.to_owned(), home.as_os_str().to_owned());
        }
    }
    let mut profile = OsString::from("--user-data-dir=");
    profile.push(root.join("داده‌های مرورگر"));
    let mut log_file = OsString::from("--log-file=");
    log_file.push(root.join("chromium-startup.log"));
    let arguments = [
        "--headless=new".into(),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--remote-debugging-address=127.0.0.1".into(),
        "--remote-debugging-port=0".into(),
        "--enable-logging".into(),
        log_file,
        profile,
        fixture.to_owned(),
    ];
    #[cfg(target_os = "linux")]
    let mut tree = OwnedProcessTree::spawn(browser, &arguments, root, &environment).unwrap();
    #[cfg(windows)]
    let tree = OwnedProcessTree::spawn(browser, &arguments, root, &environment).unwrap();
    let mut signal = [0; 1];
    let requested = std::io::stdin().read_exact(&mut signal).is_ok() && signal == *b"s";
    let browser_was_running = !tree.root_exited().unwrap();
    tree.terminate_and_wait(Duration::from_secs(8)).unwrap();
    // Windows can empty the job before signaling the browser process handle.
    let deadline = Instant::now() + Duration::from_secs(4);
    while !tree.root_exited().unwrap() {
        assert!(
            Instant::now() < deadline,
            "The browser process handle did not signal"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    if requested {
        assert!(
            browser_was_running,
            "Chromium did not stay active until requested shutdown"
        );
    }
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let role = args[0].to_str().unwrap();
    let root = Path::new(&args[1]);
    match role {
        #[cfg(any(target_os = "linux", windows))]
        "chromium-owner" => chromium_owner(root, Path::new(&args[2]), &args[3]),
        "echo" => {
            let values: Vec<_> = args[2..].iter().map(|s| s.to_str().unwrap()).collect();
            fs::write(root.join("argv.json"), serde_json::to_vec(&values).unwrap()).unwrap();
            fs::write(
                root.join("environment.json"),
                serde_json::to_vec(&serde_json::json!({
                    "marker": std::env::var("NUMERA_MARKER").ok(),
                    "parent_token_present": std::env::var_os("NUMERA_PARENT_SECRET").is_some()
                }))
                .unwrap(),
            )
            .unwrap();
        }
        "leaf" => hold(root),
        "parent" => {
            // This fixture deliberately exits before its child to test that
            // kernel-job ownership outlives the original browser parent.
            #[allow(clippy::zombie_processes)]
            let spawned = child("leaf", root).spawn().unwrap();
            await_file(&root.join("leaf-ready"));
            fs::write(root.join("parent-ready"), spawned.id().to_string()).unwrap();
            // Intentionally exit with a descendant alive. The kernel job,
            // not this parent's lifetime, must retain ownership of that child.
        }
        #[cfg(target_os = "linux")]
        "linux-detach-parent" => {
            use std::os::unix::process::CommandExt;
            #[allow(clippy::zombie_processes)]
            let spawned = child("leaf", root).process_group(0).spawn().unwrap();
            await_file(&root.join("leaf-ready"));
            fs::write(root.join("parent-ready"), spawned.id().to_string()).unwrap();
        }
        #[cfg(target_os = "linux")]
        "linux-owner" => {
            let mut tree = core_process::linux::OwnedProcessTree::spawn(
                &std::env::current_exe().unwrap(),
                &["linux-detach-parent".into(), root.as_os_str().to_owned()],
                root,
                &std::collections::BTreeMap::new(),
            )
            .unwrap();
            let start = Instant::now();
            while !tree.root_exited().unwrap() {
                assert!(
                    start.elapsed() < Duration::from_secs(5),
                    "Linux fixture parent did not exit"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            fs::write(root.join("owner-ready"), b"ready").unwrap();
            await_file(&root.join("stop"));
            tree.terminate_and_wait(Duration::from_secs(3)).unwrap();
        }
        #[cfg(target_os = "linux")]
        "linux-reject-shared" => {
            use core_contracts::ErrorCode;
            use core_process::linux::OwnedProcessTree;
            let mut existing = child("leaf", root).spawn().unwrap();
            await_file(&root.join("leaf-ready"));
            let rejected = OwnedProcessTree::spawn(
                &std::env::current_exe().unwrap(),
                &["echo".into(), root.as_os_str().to_owned()],
                root,
                &std::collections::BTreeMap::new(),
            );
            let was_alive = existing.try_wait().unwrap().is_none();
            fs::write(root.join("finish"), b"cleanup").unwrap();
            assert!(existing.wait().unwrap().success());
            assert!(matches!(rejected, Err(error) if error.code == ErrorCode::OwnershipConflict));
            assert!(was_alive);
        }
        #[cfg(target_os = "linux")]
        "linux-reject-sigchld" => {
            use core_contracts::ErrorCode;
            use core_process::linux::OwnedProcessTree;
            let rejected = OwnedProcessTree::spawn(
                &std::env::current_exe().unwrap(),
                &["echo".into(), root.as_os_str().to_owned()],
                root,
                &std::collections::BTreeMap::new(),
            );
            assert!(matches!(rejected, Err(error) if error.code == ErrorCode::OwnershipConflict));
        }
        #[cfg(windows)]
        "breakaway" => {
            use std::os::windows::process::CommandExt;
            use windows_sys::Win32::System::Threading::CREATE_BREAKAWAY_FROM_JOB;
            let allowed = match child("leaf", root)
                .creation_flags(CREATE_BREAKAWAY_FROM_JOB)
                .spawn()
            {
                Err(_) => false,
                Ok(mut escaped) => {
                    let _ = escaped.kill();
                    let _ = escaped.wait();
                    true
                }
            };
            fs::write(
                root.join("breakaway.json"),
                serde_json::to_vec(&allowed).unwrap(),
            )
            .unwrap();
        }
        #[cfg(windows)]
        "owner" => {
            use core_process::{StandaloneLease, windows::OwnedProcessTree};
            let _lease = StandaloneLease::acquire(root, "crash", "launch", "key").unwrap();
            let environment = std::env::var_os("SystemRoot")
                .map(|v| ("SystemRoot".to_owned(), v))
                .into_iter()
                .collect();
            let _tree = OwnedProcessTree::spawn(
                &std::env::current_exe().unwrap(),
                &["leaf".into(), root.as_os_str().to_owned()],
                root,
                &environment,
            )
            .unwrap();
            await_file(&root.join("leaf-ready"));
            fs::write(root.join("owner-ready"), b"ready").unwrap();
            std::thread::sleep(Duration::from_secs(30));
        }
        _ => panic!("Unknown process fixture role"),
    }
}
