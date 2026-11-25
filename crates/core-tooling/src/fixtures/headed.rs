use super::{
    Environment, Result,
    support::*,
    webdriver::{FIXTURE_TITLE, smoke_driver},
};
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
    thread::sleep,
    time::{Duration, Instant},
};

pub fn smoke_headed_browser(
    binary: &Path,
    driver: &Path,
    scratch: &Path,
    output: &Path,
    environment: &Environment,
) -> Result<Value> {
    let profile = absolute(&scratch.join("headed-browser"))?;
    let evidence = absolute(&output.join("headed-browser"))?;
    fs::create_dir(&profile)?;
    fs::create_dir(&evidence)?;
    if cfg!(windows) {
        return smoke_driver(binary, driver, &profile, &evidence, environment, true);
    }
    let display = tempfile::Builder::new()
        .prefix("numera-headed-x11-")
        .tempdir()?;
    let authority = display.path().join("Xauthority");
    let mut authority_file = OpenOptions::new();
    authority_file.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        authority_file.mode(0o600);
    }
    drop(authority_file.open(&authority)?);
    let executable = std::env::current_exe()?.with_file_name("numera-chromium-headed-fixture");
    let log = File::create(evidence.join("display.log"))?;
    let status = timed(
        command("xvfb-run", environment)
            .args(["--auto-servernum", "--auth-file"])
            .arg(&authority)
            .arg("--server-args=-screen 0 1280x900x24 -nolisten tcp")
            .arg(executable)
            .arg("under-x11")
            .arg(absolute(binary)?)
            .arg(absolute(driver)?)
            .arg(&profile)
            .arg(&evidence)
            .stdout(log.try_clone()?)
            .stderr(log),
        100,
    )?;
    if !status.success() {
        return Err(failure(
            "The Linux headed browser fixture failed; inspect headed-browser/display.log",
        ));
    }
    let report: Value = read_json(&evidence.join("report.json"))?;
    if report["result"] != "PASS"
        || report["native_window"] != "x11-viewable-window-with-browser-pid"
    {
        return Err(failure(
            "The Linux headed browser fixture did not verify a native window",
        ));
    }
    Ok(report)
}

fn matching_title(title: &str) -> bool {
    title == FIXTURE_TITLE
        || title
            .strip_prefix(FIXTURE_TITLE)
            .is_some_and(|suffix| suffix.starts_with(" - ") && suffix.len() > 3)
}

#[cfg(not(windows))]
fn probe_window(pid: u32, environment: &Environment) -> Result<bool> {
    use std::process::Stdio;
    let mut display = environment.clone();
    display.insert("LC_ALL".into(), "C.UTF-8".into());
    let tree = capture_success(
        command("xwininfo", &display).args(["-root", "-tree"]),
        3,
        1024 * 1024,
    )?;
    for line in std::str::from_utf8(&tree)?.lines() {
        let Some((id, label)) = line.trim_start().split_once(" \"") else {
            continue;
        };
        let Some((label, _)) = label.split_once("\":") else {
            continue;
        };
        if !matching_title(label)
            || !id.strip_prefix("0x").is_some_and(|digits| {
                !digits.is_empty()
                    && digits
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
        {
            continue;
        }
        let (status, owner) = capture(
            command("xprop", &display)
                .args(["-id", id, "_NET_WM_PID"])
                .stderr(Stdio::null()),
            3,
            16_384,
            false,
        )?;
        if !status.success()
            || std::str::from_utf8(&owner)?.trim() != format!("_NET_WM_PID(CARDINAL) = {pid}")
        {
            continue;
        }
        let (status, state) = capture(
            command("xwininfo", &display)
                .args(["-id", id, "-stats"])
                .stderr(Stdio::null()),
            3,
            16_384,
            false,
        )?;
        if status.success() && std::str::from_utf8(&state)?.contains("Map State: IsViewable") {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "Windows maps top-level HWNDs to PIDs through Win32 callbacks"
)]
fn probe_window(pid: u32, _: &Environment) -> Result<bool> {
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM},
        UI::WindowsAndMessaging::{
            EnumWindows, GW_OWNER, GetWindow, GetWindowTextW, GetWindowThreadProcessId,
            IsWindowVisible,
        },
    };
    use windows_sys::core::BOOL;

    struct FoundWindow {
        pid: u32,
        matches: bool,
    }

    unsafe extern "system" fn inspect(window: HWND, argument: LPARAM) -> BOOL {
        // SAFETY: EnumWindows calls synchronously with the pointer to FoundWindow below.
        let found = unsafe { &mut *(argument as *mut FoundWindow) };
        let mut owner = 0;
        // SAFETY: the enumerated window is valid for inspection and owner is writable.
        if unsafe { GetWindowThreadProcessId(window, &mut owner) } == 0 || owner != found.pid {
            return 1;
        }
        // SAFETY: these calls read properties of the enumerated HWND.
        if unsafe { IsWindowVisible(window) == 0 || !GetWindow(window, GW_OWNER).is_null() } {
            return 1;
        }
        let mut title = [0_u16; 512];
        // SAFETY: title is writable for exactly the length supplied.
        let length = unsafe { GetWindowTextW(window, title.as_mut_ptr(), title.len() as i32) };
        if length > 0 {
            found.matches = matching_title(&String::from_utf16_lossy(&title[..length as usize]));
        }
        0
    }

    let mut found = FoundWindow {
        pid,
        matches: false,
    };
    // SAFETY: the callback only accesses found during this synchronous invocation.
    unsafe {
        EnumWindows(Some(inspect), (&mut found as *mut FoundWindow) as LPARAM);
    }
    Ok(found.matches)
}

pub(super) fn verify_native_window(pid: u32, environment: &Environment) -> Result<&'static str> {
    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        if probe_window(pid, environment)? {
            return Ok(if cfg!(windows) {
                "windows-main-window-with-browser-pid"
            } else {
                "x11-viewable-window-with-browser-pid"
            });
        }
        sleep(Duration::from_millis(200));
    }
    Err(failure(
        "The headed browser did not expose the controlled native window",
    ))
}
