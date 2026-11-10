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
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let role = args[0].to_str().unwrap();
    let root = Path::new(&args[1]);
    match role {
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
