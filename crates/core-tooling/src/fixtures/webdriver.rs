//! Controlled, IPv4-loopback ChromeDriver sessions.
use super::{Environment, Result, headed::verify_native_window, support::*};
use serde_json::{Value, json};
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    path::Path,
    process::{Child, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

pub(super) const FIXTURE_TITLE: &str = "Numera source build fixture";
const RESPONSE_LIMIT: usize = 256 * 1024;
const HEAD_LIMIT: usize = 64 * 1024;
const DRIVER_ERRORS: [&str; 7] = [
    "invalid argument",
    "invalid session id",
    "no such window",
    "session not created",
    "timeout",
    "unknown error",
    "unsupported operation",
];

pub fn fixture_url() -> Result<String> {
    let file = fs::canonicalize("tests/fixtures/engine-smoke.html")?;
    url::Url::from_file_path(file)
        .map(|url| url.to_string())
        .map_err(|()| failure("Cannot construct the controlled browser fixture URL"))
}

fn read_line<R: BufRead>(reader: &mut R, limit: usize) -> Result<Vec<u8>> {
    let mut line = Vec::new();
    reader.take(limit as u64 + 1).read_until(b'\n', &mut line)?;
    if line.len() > limit || !line.ends_with(b"\n") {
        return Err(failure("Invalid HTTP fixture response headers"));
    }
    Ok(line)
}

fn read_chunks<R: BufRead>(reader: &mut R, max: usize) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut control_bytes = 0;
    loop {
        let line = read_line(reader, 1024)?;
        control_bytes += line.len();
        if control_bytes > HEAD_LIMIT {
            return Err(failure("Invalid HTTP fixture response headers"));
        }
        let line = std::str::from_utf8(&line)?.trim_end();
        let size = usize::from_str_radix(line.split(';').next().unwrap_or_default().trim(), 16)?;
        if size == 0 {
            return Ok(body);
        }
        if size > max - body.len() {
            return Err(failure(
                "Native ChromeDriver rejected the controlled command (response size limit exceeded)",
            ));
        }
        let start = body.len();
        body.resize(start + size, 0);
        reader.read_exact(&mut body[start..])?;
        let mut ending = [0; 2];
        reader.read_exact(&mut ending)?;
        if ending != *b"\r\n" {
            return Err(failure("Invalid HTTP fixture response body"));
        }
    }
}

pub(super) fn http_request(
    port: u16,
    method: &str,
    path: &str,
    data: Option<&Value>,
    timeout: u64,
    max: usize,
) -> Result<(u16, Vec<u8>)> {
    if port == 0
        || !path.starts_with('/')
        || path
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b' ')
    {
        return Err(failure("Invalid loopback fixture endpoint"));
    }
    let timeout = Duration::from_secs(timeout);
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut socket = TcpStream::connect_timeout(&address, timeout)?;
    socket.set_read_timeout(Some(timeout))?;
    socket.set_write_timeout(Some(timeout))?;
    write!(
        socket,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n"
    )?;
    if let Some(value) = data {
        let body = serde_json::to_vec(value)?;
        write!(
            socket,
            "Content-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )?;
        socket.write_all(&body)?;
    } else {
        socket.write_all(b"\r\n")?;
    }
    read_response(BufReader::new(socket), max)
}

fn read_response(mut reader: impl BufRead, max: usize) -> Result<(u16, Vec<u8>)> {
    let first = read_line(&mut reader, 1024)?;
    let first = std::str::from_utf8(&first)?;
    let mut words = first.split_ascii_whitespace();
    if !words
        .next()
        .is_some_and(|version| matches!(version, "HTTP/1.0" | "HTTP/1.1"))
    {
        return Err(failure("Invalid HTTP fixture response status"));
    }
    let code = words
        .next()
        .ok_or_else(|| failure("Invalid HTTP fixture response status"))?
        .parse::<u16>()?;
    let mut remaining = HEAD_LIMIT;
    let mut length = None;
    let mut chunked = false;
    loop {
        let line = read_line(&mut reader, remaining)?;
        remaining -= line.len();
        if matches!(line.as_slice(), b"\r\n" | b"\n") {
            break;
        }
        let (name, value) = std::str::from_utf8(&line)?
            .split_once(':')
            .ok_or_else(|| failure("Invalid HTTP fixture response headers"))?;
        if name.eq_ignore_ascii_case("content-length") {
            let next = value.trim().parse::<usize>()?;
            if length.replace(next).is_some_and(|old| old != next) {
                return Err(failure("Conflicting HTTP fixture response lengths"));
            }
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            if !value.trim().eq_ignore_ascii_case("chunked") {
                return Err(failure("Unsupported HTTP fixture transfer encoding"));
            }
            chunked = true;
        }
    }
    if chunked {
        return Ok((code, read_chunks(&mut reader, max)?));
    }
    if let Some(length) = length {
        if length > max {
            return Err(failure(
                "Native ChromeDriver rejected the controlled command (response size limit exceeded)",
            ));
        }
        let mut bytes = vec![0; length];
        reader.read_exact(&mut bytes)?;
        return Ok((code, bytes));
    }
    let mut bytes = Vec::new();
    reader.take(max as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > max {
        return Err(failure(
            "Native ChromeDriver rejected the controlled command (response size limit exceeded)",
        ));
    }
    Ok((code, bytes))
}

pub fn json_command(
    port: u16,
    method: &str,
    path: &str,
    data: Option<&Value>,
    timeout: u64,
) -> Result<Value> {
    let (status, bytes) = http_request(port, method, path, data, timeout, RESPONSE_LIMIT)?;
    decode_w3c(status, &bytes)
}

fn decode_w3c(status: u16, bytes: &[u8]) -> Result<Value> {
    let response = serde_json::from_slice::<Value>(bytes).unwrap_or(Value::Null);
    if status != 200 {
        let remote = response["value"]["error"]
            .as_str()
            .filter(|name| DRIVER_ERRORS.contains(name));
        let detail = remote.map(|name| format!(", {name}")).unwrap_or_default();
        return Err(failure(format!(
            "Native ChromeDriver rejected the controlled command (HTTP {status}{detail})"
        )));
    }
    response
        .get("value")
        .cloned()
        .ok_or_else(|| failure("Native ChromeDriver returned an invalid W3C response"))
}

fn started_port(log: &[u8]) -> Option<u16> {
    log.split(|byte| *byte == b'\n').find_map(|line| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let port = line
            .strip_prefix(b"ChromeDriver was started successfully on port ")?
            .strip_suffix(b".")?;
        decimal_port(port)
    })
}

pub(super) fn decimal_port(port: &[u8]) -> Option<u16> {
    if port.is_empty() || port.len() > 5 || port[0] == b'0' || !port.iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    std::str::from_utf8(port).ok()?.parse::<u16>().ok()
}

fn driver_version(text: &str) -> Option<&str> {
    let version = text
        .trim()
        .strip_prefix("ChromeDriver ")?
        .split_whitespace()
        .next()?;
    let mut pieces = version.splitn(4, '.');
    for _ in 0..3 {
        let part = pieces.next()?;
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
    }
    let last = pieces.next()?;
    let suffix = last.trim_start_matches(|ch: char| ch.is_ascii_digit());
    (suffix != last
        && !suffix
            .chars()
            .next()
            .is_some_and(|ch| ch.is_alphanumeric() || ch == '_'))
    .then_some(version)
}

pub struct Driver {
    child: Child,
    port: u16,
    version: String,
}

impl Driver {
    pub fn start(driver: &Path, logfile: &Path, environment: &Environment) -> Result<Self> {
        let driver = absolute(driver)?;
        let parent = driver
            .parent()
            .ok_or_else(|| failure("Missing ChromeDriver directory"))?;
        let (status, version) = capture(
            command(&driver, environment)
                .arg("--version")
                .current_dir(parent),
            10,
            16_384,
            true,
        )?;
        let version = driver_version(std::str::from_utf8(&version)?)
            .filter(|_| status.success())
            .ok_or_else(|| failure("Unrecognized native ChromeDriver version"))?
            .to_owned();
        let log = File::create(logfile)?;
        let child = command(&driver, environment)
            .arg("--port=0")
            .current_dir(parent)
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(log)
            .spawn()?;
        let mut running = Self {
            child,
            port: 0,
            version,
        };
        let deadline = Instant::now() + Duration::from_secs(25);
        while Instant::now() < deadline {
            if running.child.try_wait()?.is_some() {
                return Err(failure(
                    "Native ChromeDriver ended before exposing its loopback port",
                ));
            }
            if let Some(port) = started_port(&tail(logfile, 16_384)?) {
                let status = json_command(port, "GET", "/status", None, 15)?;
                if status["ready"] != true {
                    return Err(failure("Native ChromeDriver did not report readiness"));
                }
                running.port = port;
                return Ok(running);
            }
            sleep(Duration::from_millis(100));
        }
        Err(failure(
            "Native ChromeDriver did not expose its loopback port in time",
        ))
    }

    pub fn port(&self) -> u16 {
        self.port
    }
    pub fn version(&self) -> &str {
        &self.version
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        stop_tree(&mut self.child);
    }
}

pub(super) struct Session {
    port: u16,
    path: String,
}

impl Session {
    pub fn call(&self, method: &str, suffix: &str, value: Option<&Value>) -> Result<Value> {
        json_command(
            self.port,
            method,
            &format!("{}{suffix}", self.path),
            value,
            15,
        )
    }
}

pub(super) fn with_session<T>(
    port: u16,
    binary: &Path,
    profile: &Path,
    headed: bool,
    preferences: Option<&Value>,
    run: impl FnOnce(&Session, &Value) -> Result<T>,
) -> Result<T> {
    let mut arguments = vec![
        "--no-first-run".to_owned(),
        "--no-default-browser-check".to_owned(),
        format!("--user-data-dir={}", path_text(&absolute(profile)?)?),
    ];
    if !headed {
        arguments.insert(0, "--headless=new".to_owned());
    } else if cfg!(target_os = "linux") {
        arguments.insert(0, "--ozone-platform=x11".to_owned());
    }
    let mut options = json!({"binary":path_text(&absolute(binary)?)?,"args":arguments});
    if let Some(value) = preferences {
        options["prefs"] = value.clone();
    }
    let request = json!({"capabilities":{"alwaysMatch":{"browserName":"chrome","goog:chromeOptions":options}}});
    let data = json_command(port, "POST", "/session", Some(&request), 45)?;
    let id = data
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|id| (32..=64).contains(&id.len()) && id.bytes().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| failure("Native ChromeDriver returned an invalid session identifier"))?;
    let session = Session {
        port,
        path: format!("/session/{id}"),
    };
    let result = run(&session, &data);
    let shutdown = json_command(
        port,
        "DELETE",
        &session.path,
        None,
        if result.is_ok() { 10 } else { 5 },
    );
    result.and_then(|value| shutdown.map(|_| value))
}

pub fn smoke_driver(
    binary: &Path,
    driver: &Path,
    scratch: &Path,
    output: &Path,
    environment: &Environment,
    headed: bool,
) -> Result<Value> {
    let driver = Driver::start(
        driver,
        &output.join("chromedriver-fixture.log"),
        environment,
    )?;
    with_session(
        driver.port(),
        binary,
        &scratch.join("driver-fixture-profile"),
        headed,
        None,
        |session, data| {
            session.call("POST", "/url", Some(&json!({"url":fixture_url()?})))?;
            let body =
                json!({"script":"return document.getElementById('result').textContent;","args":[]});
            if session.call("POST", "/execute/sync", Some(&body))? != "NUMERA_RENDERER_FIXTURE_PASS"
            {
                return Err(failure(
                    "Native WebDriver did not observe the renderer fixture marker",
                ));
            }
            let mut report = json!({"result":"PASS","driver_version":driver.version(),
            "scope":"ChromeDriver W3C HTTP session and a controlled local document"});
            if headed {
                let pid = data["capabilities"]["goog:processID"]
                    .as_u64()
                    .and_then(|pid| u32::try_from(pid).ok())
                    .filter(|pid| *pid > 0)
                    .ok_or_else(|| {
                        failure("Native ChromeDriver did not identify the headed browser process")
                    })?;
                report["native_window"] = json!(verify_native_window(pid, environment)?);
                report["scope"] =
                    json!("Headed W3C navigation and an OS window owned by the browser process");
            }
            Ok(report)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn mock_response(response: &[u8]) -> Result<Value> {
        let (status, body) = read_response(Cursor::new(response), RESPONSE_LIMIT)?;
        decode_w3c(status, &body)
    }

    #[test]
    fn driver_banner_rejects_invalid_ports_and_incomplete_lines() {
        assert_eq!(
            driver_version("ChromeDriver 157.0.8103.79 (commit)"),
            Some("157.0.8103.79")
        );
        assert_eq!(driver_version("ChromeDriver 157.0.8103.79xyz"), None);
        assert_eq!(
            started_port(b"some\r\nChromeDriver was started successfully on port 12345.\r\n"),
            Some(12345)
        );
        for log in [
            &b"ChromeDriver was started successfully on port 65536.\n"[..],
            &b"ChromeDriver was started successfully on port 0.\n"[..],
            &b"ChromeDriver was started successfully on port 8888"[..],
        ] {
            assert_eq!(started_port(log), None);
        }
    }

    #[test]
    fn validates_w3c_response_and_rejects_redirects_and_excess_bytes() -> Result<()> {
        let valid = b"HTTP/1.0 200 OK\r\nContent-Length: 24\r\n\r\n{\"value\":{\"ready\":true}}";
        assert_eq!(mock_response(valid)?["ready"], true);
        for response in [
            &b"HTTP/1.0 302 Found\r\nLocation: /status\r\nContent-Length: 0\r\n\r\n"[..],
            &b"HTTP/1.0 200 OK\r\nContent-Length: 262145\r\n\r\n"[..],
        ] {
            assert!(mock_response(response).is_err());
        }
        Ok(())
    }
}
