#[path = "../src/bin/numera-automation-baseline/contract.rs"]
mod contract;

use serde_json::json;
use std::path::Path;

#[test]
fn accepts_only_original_loopback_endpoint_grammar() {
    for (input, expected) in [
        ("localhost:53124", "http://127.0.0.1:53124"),
        ("127.0.0.1:65535", "http://127.0.0.1:65535"),
        ("localhost:1", "http://127.0.0.1:1"),
    ] {
        assert_eq!(
            contract::loopback_port(&json!(input)).as_deref(),
            Ok(expected)
        );
    }
    for input in [
        json!(null),
        json!(123),
        json!(false),
        json!([]),
        json!({}),
        json!("0.0.0.0:53124"),
        json!("localhost.example:80"),
        json!("localhost:65536"),
        json!("localhost:0"),
        json!("localhost:80\n"),
        json!("http://localhost:80"),
        json!("127.0.0.1:80@external.test"),
        json!("localhost:01"),
        json!("localhost:１２"),
        json!("localhost:+80"),
        json!("[::1]:80"),
    ] {
        assert_eq!(
            contract::loopback_port(&input).unwrap_err(),
            contract::ENDPOINT_ERROR
        );
    }
}

#[test]
fn sdk_report_requires_every_real_client_scenario_and_exact_versions() {
    let valid = example_sdk_report();
    assert!(contract::validate_sdk_report(&valid).is_ok());
    for pointer in [
        "/node",
        "/browserVersion",
        "/tests/selenium/version",
        "/tests/puppeteer/transport",
        "/tests/playwright/checks/4",
        "/responsiveViewport/reload",
        "/responsiveViewport/separate_target",
        "/responsiveViewport/layouts/0/width",
        "/responsiveViewport/layouts/1/deviceScaleFactor",
    ] {
        let mut changed = valid.clone();
        *changed.pointer_mut(pointer).expect("fixture field") = json!("wrong");
        assert!(
            contract::validate_sdk_report(&changed).is_err(),
            "accepted {pointer}"
        );
    }
    let mut changed = valid;
    changed["tests"]["puppeteer"]["checks"] = json!(["navigation"]);
    assert!(contract::validate_sdk_report(&changed).is_err());
}

#[test]
fn missing_sdk_adapter_is_explicitly_not_run() {
    let value = contract::sdk_not_run();
    assert_eq!(value["result"], "NOT_RUN");
    assert_eq!(value["sdk_compatibility_verified"], false);
}

#[test]
fn token_and_runtime_override_environment_is_removed() {
    let input = [
        ("gH_tOkEn".into(), "secret".into()),
        ("NODE_OPTIONS".into(), "bad".into()),
        ("HTTPS_PROXY".into(), "bad".into()),
        ("PATH".into(), "tools".into()),
    ]
    .into_iter()
    .collect();
    let output = contract::child_environment(input);
    assert_eq!(output.len(), 2);
    assert_eq!(
        output
            .get(std::ffi::OsStr::new("PATH"))
            .and_then(|v| v.to_str()),
        Some("tools")
    );
    assert_eq!(
        output
            .get(std::ffi::OsStr::new("NO_PROXY"))
            .and_then(|v| v.to_str()),
        Some("localhost,127.0.0.1")
    );
}

#[test]
fn actual_binary_preserves_loopback_results_and_unicode_rejection() {
    for (input, expected) in [
        (
            json!("localhost:53124"),
            json!({"ok":true,"value":"http://127.0.0.1:53124"}),
        ),
        (
            json!("localhost:１２"),
            json!({"ok":false,"error":contract::ENDPOINT_ERROR}),
        ),
        (
            json!(null),
            json!({"ok":false,"error":contract::ENDPOINT_ERROR}),
        ),
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_numera-automation-baseline"))
            .arg("--loopback")
            .arg(input.to_string())
            .output()
            .expect("run baseline parser");
        assert!(output.status.success());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output.stdout).expect("JSON output"),
            expected
        );
    }
}

#[test]
fn actual_sdk_command_without_adapter_never_claims_sdk_coverage() {
    let temporary = tempfile::tempdir().expect("owned SDK fixture directory");
    let binary = env!("CARGO_BIN_EXE_numera-automation-baseline");
    let output = std::process::Command::new(binary)
        .arg("--sdk-clients")
        .args([
            Path::new(binary),
            Path::new(binary),
            temporary.path(),
            temporary.path(),
        ])
        .output()
        .expect("run missing adapter fixture");
    assert!(output.status.success());
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("SDK status JSON");
    assert_eq!(report["result"], "NOT_RUN");
    assert_eq!(report["sdk_compatibility_verified"], false);
    assert_eq!(
        std::fs::read_dir(temporary.path())
            .expect("inspect owned directory")
            .count(),
        0
    );
}

#[test]
fn missing_local_browser_retains_blocked_report_without_sdk_claims() {
    if !cfg!(any(target_os = "windows", target_os = "linux")) {
        return;
    }
    let temporary = tempfile::tempdir().expect("owned temporary directory");
    let output = temporary.path().join("evidence مسیر");
    let scratch = temporary.path().join("profile space");
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_numera-automation-baseline"))
        .args(["--local", "--browser"])
        .arg(temporary.path().join("missing-browser"))
        .arg("--driver")
        .arg(temporary.path().join("missing-driver"))
        .arg("--output")
        .arg(&output)
        .arg("--scratch")
        .arg(&scratch)
        .output()
        .expect("run missing input fixture");
    assert_eq!(result.status.code(), Some(2));
    let report: serde_json::Value = serde_json::from_slice(
        &std::fs::read(output.join("report.json")).expect("blocked evidence"),
    )
    .expect("JSON report");
    assert_eq!(report["result"], "BLOCKED");
    assert_eq!(report["source_build"], "NOT_RUN");
    assert_eq!(report["owned_engine_used"], false);
    assert!(report.get("automation_clients").is_none());
}

fn example_sdk_report() -> serde_json::Value {
    json!({"result":"PASS","node":"24.22.3","browserVersion":"150.0.1.2",
        "tests":{
            "selenium":{"version":"4.53.0","transport":"WebDriver","checks":["navigation","unicode-input","click-and-wait","nested-frame","png-screenshot"]},
            "puppeteer":{"version":"25.18.1","transport":"Chrome DevTools Protocol","checks":["navigation","unicode-input","click-and-wait","nested-frame","png-screenshot"]},
            "playwright":{"version":"1.65.1","transport":"connectOverCDP","checks":["navigation","unicode-input","click-and-wait","nested-frame","png-screenshot"]}},
        "responsiveViewport":{"result":"PASS","reload":"PASS","separate_target":"PASS","layouts":[
            {"width":390,"height":844,"screenWidth":390,"screenHeight":844,"deviceScaleFactor":3,"touchPoints":5},
            {"width":844,"height":390,"screenWidth":844,"screenHeight":390,"deviceScaleFactor":2,"touchPoints":5}]}})
}
