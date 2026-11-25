//! Check the existing native owner against a real browser and its loopback endpoint.
use super::{
    Environment, Result,
    support::*,
    webdriver::{FIXTURE_TITLE, decimal_port, fixture_url, http_request},
};
use serde_json::{Value, json};
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    thread::sleep,
    time::{Duration, Instant},
};

pub fn local_file_url_path(value: &str) -> Option<PathBuf> {
    let url = url::Url::parse(value).ok()?;
    if url.scheme() != "file"
        || !matches!(url.host_str(), None | Some("localhost"))
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    url.to_file_path().ok()
}

fn same_path(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        return left.to_string_lossy().to_uppercase() == right.to_string_lossy().to_uppercase();
    }
    left == right
}

fn observe_browser(port_file: &Path, fixture: &str) -> (&'static str, bool) {
    let mut first_line = Vec::new();
    match File::open(port_file) {
        Ok(file) => {
            if BufReader::new(file)
                .take(8)
                .read_until(b'\n', &mut first_line)
                .is_err()
            {
                return ("debugger port file was unreadable", false);
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ("waiting for the debugger port file", false);
        }
        Err(_) => return ("debugger port file was unreadable", false),
    }
    let Some(port) = decimal_port(first_line.trim_ascii()) else {
        return ("debugger port file held an invalid port", false);
    };
    let Ok((status, bytes)) = http_request(port, "GET", "/json/list", None, 2, 65_536) else {
        return ("debugger did not return usable target data", false);
    };
    if status != 200 {
        return ("debugger returned an invalid target response", false);
    }
    let Ok(targets) = serde_json::from_slice::<Value>(&bytes) else {
        return ("debugger did not return usable target data", false);
    };
    if let Some(expected) = local_file_url_path(fixture) {
        for target in targets.as_array().into_iter().flatten() {
            if target["type"] == "page"
                && target["title"] == FIXTURE_TITLE
                && let Some(current) = target["url"].as_str()
                && local_file_url_path(current).is_some_and(|path| same_path(&path, &expected))
            {
                return ("rendered", current != fixture);
            }
        }
    }
    ("debugger responded without the controlled page", false)
}

fn capture_startup_sites(source: &Path, output: &Path) -> Result<()> {
    if !source.is_file() {
        return Ok(());
    }
    let mut records = Vec::new();
    for line in String::from_utf8_lossy(&tail(source, 64 * 1024)?).lines() {
        for level in ["WARNING", "ERROR", "FATAL"] {
            let Some((_, location)) = line.split_once(&format!(":{level}:")) else {
                continue;
            };
            let Some((location, _)) = location.split_once(']') else {
                continue;
            };
            let Some((file, line)) = location.rsplit_once(':') else {
                continue;
            };
            if !file.is_empty()
                && file.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'/' | b'\\')
                })
                && let Ok(number) = line.parse::<u64>()
            {
                records.push(json!({"level":level,"location":file,"line":number}));
            }
        }
    }
    write_json(
        &output.join("chromium-startup-sites.json"),
        &records[records.len().saturating_sub(30)..].to_vec(),
    )
}

fn capture_process_shape(pid: u32, profile: &Path, output: &Path) -> Result<()> {
    let mut counts = std::collections::BTreeMap::from([
        ("browser", 0_u64),
        ("renderer", 0),
        ("gpu-process", 0),
        ("utility", 0),
        ("other", 0),
    ]);
    let mut root_arguments = Value::Null;
    for process in descendants(pid) {
        #[cfg(not(windows))]
        let arguments = process_arguments(process.pid);
        #[cfg(windows)]
        let arguments = windows_arguments(&process);
        let fallback = if process.parent == pid && process.name.eq_ignore_ascii_case("chrome.exe") {
            "browser"
        } else {
            "other"
        };
        let role = arguments.as_ref().map_or(fallback, |args| {
            args.iter()
                .find_map(|arg| arg.strip_prefix("--type="))
                .unwrap_or("browser")
        });
        if role == "browser"
            && process.parent == pid
            && let Some(arguments) = &arguments
        {
            root_arguments = json!({
                "headless":arguments.iter().any(|value| value == "--headless=new"),
                "auto_debugging_port":arguments.iter().any(|value| value == "--remote-debugging-port=0"),
                "explicit_profile":arguments.iter().filter_map(|value| value.strip_prefix("--user-data-dir="))
                    .any(|value| same_path(Path::new(value), profile)),
            });
        }
        let role = if counts.contains_key(role) {
            role
        } else {
            "other"
        };
        if let Some(total) = counts.get_mut(role) {
            *total += 1;
        }
    }
    write_json(
        &output.join("chromium-ownership-startup.json"),
        &json!({
            "process_roles":counts, "browser_arguments":root_arguments,
            "profile_created":profile.is_dir(),"local_state_created":profile.join("Local State").is_file(),
            "debugger_port_created":profile.join("DevToolsActivePort").is_file()
        }),
    )
}

#[cfg(target_os = "linux")]
fn process_arguments(pid: u32) -> Option<Vec<String>> {
    let file = format!("/proc/{pid}/cmdline");
    let bytes = read_bounded(Path::new(&file), 128 * 1024).ok()?;
    Some(
        bytes
            .split(|byte| *byte == 0)
            .skip(1)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect(),
    )
}

#[cfg(not(any(target_os = "linux", windows)))]
fn process_arguments(_: u32) -> Option<Vec<String>> {
    None
}

pub fn smoke_browser_ownership(
    binary: &Path,
    scratch: &Path,
    output: &Path,
    environment: &Environment,
) -> Result<Value> {
    let supervisor = fs::canonicalize(Path::new("target/debug").join(format!(
        "numera-process-fixture{}",
        std::env::consts::EXE_SUFFIX
    )))?;
    let root = absolute(&scratch.join("Rust browser ownership"))?;
    fs::create_dir(&root)?;
    let fixture = fixture_url()?;
    let profile = root.join("داده‌های مرورگر");
    let port_file = profile.join("DevToolsActivePort");
    let log = File::create(output.join("chromium-ownership.log"))?;
    let mut child = command(supervisor, environment)
        .arg("chromium-owner")
        .arg(&root)
        .arg(absolute(binary)?)
        .arg(&fixture)
        .stdin(Stdio::piped())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()?;
    let result = (|| -> Result<Value> {
        let deadline = Instant::now() + Duration::from_secs(45);
        let (mut observation, mut equivalent) = observe_browser(&port_file, &fixture);
        while observation != "rendered" {
            if child.try_wait()?.is_some() || Instant::now() >= deadline {
                capture_process_shape(child.id(), &profile, output)?;
                return Err(failure(format!(
                    "The Rust ownership fixture did not observe the controlled page ({observation})"
                )));
            }
            sleep(Duration::from_millis(100));
            (observation, equivalent) = observe_browser(&port_file, &fixture);
        }
        let observed = descendants(child.id());
        if observed.len() < 2 {
            return Err(failure(
                "The Chromium ownership fixture did not observe a browser tree",
            ));
        }
        let signal = child
            .stdin
            .as_mut()
            .ok_or_else(|| failure("Missing browser ownership shutdown pipe"))?;
        signal.write_all(b"s")?;
        signal.flush()?;
        if !wait(&mut child, Duration::from_secs(16))?.is_some_and(|status| status.success()) {
            return Err(failure(
                "The Rust ownership fixture could not stop the Chromium tree",
            ));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while !observed.iter().all(exited) {
            if Instant::now() >= deadline {
                return Err(failure(
                    "Observed Chromium processes remained after fixture shutdown",
                ));
            }
            sleep(Duration::from_millis(50));
        }
        Ok(
            json!({"result":"PASS","rendered":"PASS","owned_tree_shutdown":"PASS",
            "observed_processes":observed.len(),"profile_path":"Unicode with spaces",
            "normalized_file_url":equivalent}),
        )
    })();
    drop(child.stdin.take());
    if !matches!(wait(&mut child, Duration::from_secs(10)), Ok(Some(_))) {
        stop_tree(&mut child);
    }
    let startup = capture_startup_sites(&root.join("chromium-startup.log"), output);
    result.and_then(|report| startup.map(|()| report))
}
