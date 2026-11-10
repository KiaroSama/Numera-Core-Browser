use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Command, Stdio},
};

fn run(input: &[u8]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_numera-core"))
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}
fn frames(output: &std::process::Output) -> Vec<Value> {
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}
#[test]
fn real_producer_negotiates_and_reports_no_installed_browser() {
    let mut input = include_bytes!("../../../contracts/fixtures/hello.json").to_vec();
    input.extend_from_slice(include_bytes!(
        "../../../contracts/fixtures/capabilities.json"
    ));
    input.extend_from_slice(b"{\"namespace\":\"numera.core\",\"protocol\":1,\"id\":\"list-1\",\"deadline_ms\":30000,\"operation\":\"engines.list\",\"payload\":{}}\n");
    let out = run(&input);
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    let f = frames(&out);
    assert_eq!(f.len(), 3);
    assert_eq!(f[0]["id"], "hello-1");
    assert_eq!(
        f[1]["result"]["features"]["browser.gui"]["status"],
        "unsupported"
    );
    assert_eq!(f[2]["result"]["installed"], json!([]));
}
#[test]
fn clean_eof_exits_without_a_response_or_background_session() {
    let out = run(b"");
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}
#[test]
fn incomplete_or_invalid_utf8_frames_fail_with_clean_json() {
    for input in [b"{}".as_slice(), b"\xff\n".as_slice()] {
        let out = run(input);
        let f = frames(&out);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0]["status"], "error");
    }
    assert_eq!(run(b"{}").status.code(), Some(2));
}
#[test]
fn duplicate_nested_keys_rejected_by_actual_process() {
    let out = run(include_bytes!(
        "../../../contracts/fixtures/negative-duplicate-key.ndjson"
    ));
    assert_eq!(frames(&out)[0]["error"]["code"], "invalid_configuration");
}
#[test]
fn parser_output_never_contains_supplied_synthetic_secret() {
    let out = run(b"{\"password\":\"synthetic-super-secret\",\"password\":null}\n");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("synthetic-super-secret"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("synthetic-super-secret"));
}
#[test]
fn cli_doctor_runs_without_display_or_client_runtimes() {
    let out = Command::new(env!("CARGO_BIN_EXE_numera-core"))
        .args(["--json", "doctor"])
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(out.status.success());
    let d: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(d["owned_engine_available"], false);
    assert_eq!(d["python_required"], false);
    assert_eq!(d["node_required"], false);
    assert!(out.stderr.is_empty());
}
#[test]
fn oversized_frame_is_fatal_and_bounded() {
    // Write asynchronously: the server is allowed to close while input is sent.
    let mut child = Command::new(env!("CARGO_BIN_EXE_numera-core"))
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || {
        let _ = input.write_all(&vec![b'x'; core_contracts::MAX_FRAME + 1]);
    });
    let out = child.wait_with_output().unwrap();
    writer.join().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.len() < 2048);
}

#[test]
fn stalled_protocol_reader_cannot_block_server_forever() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_numera-core"))
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || {
        if input
            .write_all(include_bytes!("../../../contracts/fixtures/hello.json"))
            .is_err()
        {
            return;
        }
        for id in 0..800 {
            let frame = json!({"namespace":"numera.core","protocol":1,"id":format!("backpressure-{id}"),"deadline_ms":30000,"operation":"capabilities.get","payload":{}});
            if writeln!(input, "{frame}").is_err() {
                break;
            }
        }
    });
    let start = std::time::Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if start.elapsed() > std::time::Duration::from_secs(15) {
            child.kill().unwrap();
            writer.join().unwrap();
            panic!("Protocol output deadline was not enforced");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let out = child.wait_with_output().unwrap();
    writer.join().unwrap();
    assert_eq!(out.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&out.stderr).contains("backpressure timeout"));
}
