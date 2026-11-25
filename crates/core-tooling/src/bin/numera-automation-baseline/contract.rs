use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
};

pub const ENDPOINT_ERROR: &str = "Expected a local Chromium automation endpoint";

pub fn loopback_port(address: &Value) -> Result<String, String> {
    let invalid = || ENDPOINT_ERROR.to_owned();
    let text = address.as_str().ok_or_else(invalid)?;
    let port = text
        .strip_prefix("localhost:")
        .or_else(|| text.strip_prefix("127.0.0.1:"))
        .ok_or_else(invalid)?;
    if port.is_empty()
        || port.len() > 5
        || port.starts_with('0')
        || !port.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(invalid());
    }
    if port.parse::<u32>().map_err(|_| invalid())? > 65535 {
        return Err(invalid());
    }
    Ok(format!("http://127.0.0.1:{port}"))
}

pub fn child_environment(input: BTreeMap<OsString, OsString>) -> BTreeMap<OsString, OsString> {
    let mut output: BTreeMap<_, _> = input
        .into_iter()
        .filter(|(key, _)| {
            ![
                "GH_TOKEN",
                "GITHUB_TOKEN",
                "ACTIONS_RUNTIME_TOKEN",
                "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
                "ACTIONS_ID_TOKEN_REQUEST_URL",
                "NODE_OPTIONS",
                "NODE_PATH",
                "NODE_USE_ENV_PROXY",
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "NO_PROXY",
            ]
            .iter()
            .any(|name| key.eq_ignore_ascii_case(OsStr::new(name)))
        })
        .collect();
    output.insert("NO_PROXY".into(), "localhost,127.0.0.1".into());
    output
}

pub fn sdk_not_run() -> Value {
    json!({"result":"NOT_RUN", "sdk_compatibility_verified":false,
        "reason":"No explicit external SDK adapter was supplied",
        "scope":"Real Playwright, Puppeteer and Selenium client scenarios require their external SDK runner"})
}

pub fn validate_sdk_report(report: &Value) -> Result<(), String> {
    let error =
        || "External SDK adapter did not prove the complete pinned client scenarios".to_owned();
    if report["result"] != "PASS" || report["node"] != "24.22.3" {
        return Err(error());
    }
    let version = report["browserVersion"].as_str().ok_or_else(error)?;
    let parts: Vec<_> = version.split('.').collect();
    if parts.len() != 4
        || parts
            .iter()
            .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(error());
    }
    let checks = json!([
        "navigation",
        "unicode-input",
        "click-and-wait",
        "nested-frame",
        "png-screenshot"
    ]);
    for (name, version, transport) in [
        ("selenium", "4.53.0", "WebDriver"),
        ("puppeteer", "25.18.1", "Chrome DevTools Protocol"),
        ("playwright", "1.65.1", "connectOverCDP"),
    ] {
        let client = &report["tests"][name];
        if client["version"] != version
            || client["transport"] != transport
            || client["checks"] != checks
        {
            return Err(error());
        }
    }
    let viewport = &report["responsiveViewport"];
    let expected = json!([
        {"width":390,"height":844,"screenWidth":390,"screenHeight":844,"deviceScaleFactor":3,"touchPoints":5},
        {"width":844,"height":390,"screenWidth":844,"screenHeight":390,"deviceScaleFactor":2,"touchPoints":5}
    ]);
    if viewport["result"] != "PASS"
        || viewport["reload"] != "PASS"
        || viewport["separate_target"] != "PASS"
        || viewport["layouts"] != expected
    {
        return Err(error());
    }
    Ok(())
}
