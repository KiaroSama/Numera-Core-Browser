#![cfg(target_os = "linux")]
#[path = "../../core-engines/tests/support/mod.rs"]
mod signed_fixture;

use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{self, Stdio},
    time::{Duration, Instant},
};

fn call(
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    id: u32,
    operation: &str,
    payload: Value,
) -> Value {
    let message = json!({"namespace":"numera.core","protocol":1,"id":id.to_string(),"deadline_ms":30000,"operation":operation,"payload":payload});
    writeln!(writer, "{message}").unwrap();
    writer.flush().unwrap();
    let mut response = String::new();
    reader.read_line(&mut response).unwrap();
    serde_json::from_str(&response)
        .unwrap_or_else(|_| panic!("Invalid Core response for {operation}"))
}

fn start(store: &Path, root: &Path) -> process::Child {
    process::Command::new(env!("CARGO_BIN_EXE_numera-core"))
        .arg("serve")
        .args([store, root])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap()
}

fn wait(child: &mut process::Child) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "Core must shut down after EOF");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn signed_package_reaches_native_session_and_eof_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("userdata");
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(&state).unwrap();
    let publish = root.path().join("published");
    let trust_root = root.path().join("root.json");
    let key = signed_fixture::Key::new(16);
    fs::write(&trust_root, key.root(1, &[&key])).unwrap();
    let executable = b"#!/bin/sh\nfor arg in \"$@\"; do case \"$arg\" in --user-data-dir=*) data=${arg#*=};; esac; done\nprintf 'test profile' > \"$data/preferences\"\nexec /bin/sleep 25\n";
    let (digest, _) =
        signed_fixture::repository(&publish, &key, 1, executable, signed_fixture::EXPIRES);
    let store = root.path().join("store");
    let cli = env!("CARGO_BIN_EXE_numera-core");
    let install = process::Command::new(cli)
        .args(["engines", "install"])
        .args([&store, &trust_root, &publish])
        .output()
        .unwrap();
    assert!(
        install.status.success(),
        "{}",
        String::from_utf8_lossy(&install.stdout)
    );
    let mut desired: Value = serde_json::from_str(include_str!(
        "../../../contracts/fixtures/launch-desktop.json"
    ))
    .unwrap();
    desired["resources"]["data_root"] = json!(state);
    desired["resources"]["native_profile_dir"] = json!(state.join("browser"));
    desired["resources"]["cache_root"] = json!(state.join("cache"));
    let setup = json!({"protocols":{"min":1,"max":1},"schemas":{"min":1,"max":1}});

    let mut child = start(&store, &state);
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    assert_eq!(
        call(&mut stdout, &mut stdin, 1, "hello", setup.clone())["status"],
        "ok"
    );
    assert_eq!(
        call(&mut stdout, &mut stdin, 2, "capabilities.get", json!({}))["result"]["scope"]["engine_build"],
        digest
    );
    assert_eq!(
        call(&mut stdout, &mut stdin, 3, "launch.plan", desired.clone())["result"]["status"],
        "planned"
    );
    assert!(!state.join("browser").exists());
    let running = call(
        &mut stdout,
        &mut stdin,
        4,
        "session.launch",
        desired.clone(),
    );
    assert_eq!(running["result"]["state"], "running", "{running}");
    assert_eq!(running["result"]["package_sha256"], digest);
    assert_eq!(
        call(
            &mut stdout,
            &mut stdin,
            5,
            "session.launch",
            desired.clone()
        ),
        json!({"namespace":"numera.core","protocol":1,"id":"5","status":"ok","result":running["result"]})
    );
    assert_eq!(
        call(
            &mut stdout,
            &mut stdin,
            6,
            "session.events",
            json!({"launch_id":"fixture-launch","after_sequence":0})
        )["result"]["next_sequence"],
        1
    );
    drop(stdin);
    wait(&mut child);
    let source = state.join("browser");
    let backup = state.join("backup");
    let copy = process::Command::new(cli)
        .args(["profiles", "snapshot"])
        .args([&state, &source, &backup])
        .output()
        .unwrap();
    assert!(
        copy.status.success(),
        "{}",
        String::from_utf8_lossy(&copy.stdout)
    );
    assert_eq!(
        fs::read(source.join("preferences")).unwrap(),
        b"test profile"
    );
    assert_eq!(
        fs::read(backup.join("preferences")).unwrap(),
        b"test profile"
    );

    let mut child = start(&store, &state);
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    call(&mut stdout, &mut stdin, 1, "hello", setup);
    call(&mut stdout, &mut stdin, 2, "capabilities.get", json!({}));
    let repeated = call(&mut stdout, &mut stdin, 3, "session.launch", desired);
    assert_eq!(
        repeated["error"]["code"], "ownership_conflict",
        "{repeated}"
    );
    drop(stdin);
    wait(&mut child);
}
