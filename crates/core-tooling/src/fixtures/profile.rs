//! Browser restart, storage isolation, worker and download fixture.
use super::{
    Environment, Result,
    support::*,
    webdriver::{Driver, Session, with_session},
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const MARKER: &str = "Numera Persian داده‌های مرورگر";
const DOWNLOAD: &[u8] = b"Numera controlled browser download\x00\xff\n";
const PAGE: &[u8] = b"<!doctype html><meta charset=\"utf-8\"><title>Numera profile fixture</title>\n<style>#sample { color: rgb(23, 45, 67) }</style>\n<p id=\"sample\">Profile storage fixture</p><a id=\"download\" href=\"/data\">Download</a>";
const WORKER: &[u8] = b"self.onmessage = event => self.postMessage('worker:' + event.data);";

fn serve(mut connection: TcpStream, expected_host: &str) -> io::Result<()> {
    connection.set_read_timeout(Some(Duration::from_secs(2)))?;
    connection.set_write_timeout(Some(Duration::from_secs(2)))?;
    let (method, route, host) = {
        let mut reader = BufReader::new(&mut connection).take(16 * 1024);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let mut request = line.split_ascii_whitespace();
        let method = request.next().unwrap_or_default().to_owned();
        let route = request.next().unwrap_or_default().to_owned();
        let mut host = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("host")
            {
                host = value.trim().to_owned();
            }
        }
        (method, route, host)
    };
    let (status, media, data) = if method != "GET" || host != expected_host {
        ("400 Bad Request", "text/plain", &b""[..])
    } else {
        match route.as_str() {
            "/" => ("200 OK", "text/html; charset=utf-8", PAGE),
            "/worker.js" => ("200 OK", "text/javascript; charset=utf-8", WORKER),
            "/data" => ("200 OK", "application/octet-stream", DOWNLOAD),
            _ => ("404 Not Found", "text/plain", &b""[..]),
        }
    };
    write!(
        connection,
        "HTTP/1.0 {status}\r\nContent-Type: {media}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n",
        data.len()
    )?;
    if status == "200 OK" && route == "/data" {
        connection
            .write_all(b"Content-Disposition: attachment; filename=\"numera-download.bin\"\r\n")?;
    }
    connection.write_all(b"\r\n")?;
    connection.write_all(data)
}

struct Page {
    url: String,
    stop: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
}

impl Page {
    fn start() -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let host = format!("127.0.0.1:{}", listener.local_addr()?.port());
        let url = format!("http://{host}/");
        let stop = Arc::new(AtomicBool::new(false));
        let closed = Arc::clone(&stop);
        let server = thread::spawn(move || {
            thread::scope(|scope| {
                let mut active = Vec::<thread::ScopedJoinHandle<()>>::new();
                while !closed.load(Ordering::Acquire) {
                    active.retain(|handle| !handle.is_finished());
                    match listener.accept() {
                        Ok((socket, _)) if active.len() < 16 => {
                            let host = &host;
                            active.push(scope.spawn(move || {
                                let _ = serve(socket, host);
                            }));
                        }
                        Ok(_) => {}
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(_) => break,
                    }
                }
            })
        });
        Ok(Self {
            url,
            stop,
            server: Some(server),
        })
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

fn read_marker(session: &Session, page: &Page) -> Result<Value> {
    session.call("POST", "/url", Some(&json!({"url":page.url})))?;
    session.call(
        "POST",
        "/execute/sync",
        Some(&json!({
            "script":"return localStorage.getItem('numera-fixture');","args":[]
        })),
    )
}

fn check_download(session: &Session, directory: &Path) -> Result<()> {
    let element = session.call(
        "POST",
        "/element",
        Some(&json!({"using":"css selector","value":"#download"})),
    )?;
    let id = element["element-6066-11e4-a52e-4f735466cecf"]
        .as_str()
        .filter(|id| {
            (1..=160).contains(&id.len())
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
        })
        .ok_or_else(|| failure("Native WebDriver did not find the fixture download link"))?;
    session.call("POST", &format!("/element/{id}/click"), Some(&json!({})))?;
    let destination = directory.join("numera-download.bin");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !destination.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(100));
    }
    if !destination.is_file()
        || read_bounded(&destination, DOWNLOAD.len() as u64)
            .ok()
            .as_deref()
            != Some(DOWNLOAD)
    {
        return Err(failure(
            "The controlled browser download did not produce the exact fixture bytes",
        ));
    }
    Ok(())
}

pub fn smoke_profile_data(
    binary: &Path,
    driver: &Path,
    scratch: &Path,
    output: &Path,
    environment: &Environment,
) -> Result<Value> {
    let profile = scratch.join("Numera profile داده‌ها");
    let independent = scratch.join("Independent profile داده‌ها");
    let downloads = absolute(&scratch.join("Numera downloads داده‌ها"))?;
    fs::create_dir(&downloads)?;
    let preferences = json!({"download.default_directory":path_text(&downloads)?,"download.prompt_for_download":false});
    let page = Page::start()?;
    let driver = Driver::start(driver, &output.join("profile-data-driver.log"), environment)?;
    with_session(
        driver.port(),
        binary,
        &profile,
        false,
        Some(&preferences),
        |session, _| {
            if !read_marker(session, &page)?.is_null() {
                return Err(failure(
                    "The new profile already contained the fixture marker",
                ));
            }
            let color = session.call("POST", "/execute/sync", Some(&json!({"args":[MARKER],"script":
            "localStorage.setItem('numera-fixture', arguments[0]); return getComputedStyle(document.getElementById('sample')).color;"})))?;
            if color != "rgb(23, 45, 67)" {
                return Err(failure("The browser did not apply the fixture style"));
            }
            let worker = session.call(
                "POST",
                "/execute/async",
                Some(&json!({"args":[MARKER],"script":r"
            const done = arguments[arguments.length - 1];
            const worker = new Worker('/worker.js');
            worker.onerror = () => { worker.terminate(); done(null); };
            worker.onmessage = event => { worker.terminate(); done(event.data); };
            worker.postMessage(arguments[0]);
        " })),
            )?;
            if worker != format!("worker:{MARKER}") {
                return Err(failure(
                    "The dedicated worker did not return the exact Unicode marker",
                ));
            }
            check_download(session, &downloads)
        },
    )?;
    with_session(
        driver.port(),
        binary,
        &profile,
        false,
        None,
        |session, _| {
            if read_marker(session, &page)? != MARKER {
                return Err(failure(
                    "The browser profile lost its marker after a new native session",
                ));
            }
            Ok(())
        },
    )?;
    with_session(
        driver.port(),
        binary,
        &independent,
        false,
        None,
        |session, _| {
            if !read_marker(session, &page)?.is_null() {
                return Err(failure(
                    "The browser shared the marker with an independent profile",
                ));
            }
            Ok(())
        },
    )?;
    Ok(
        json!({"result":"PASS","computed_style":"PASS","dedicated_worker":"PASS",
        "download_bytes":DOWNLOAD.len(),"profile_restart":"PASS","profile_isolation":"PASS",
        "profile_and_download_paths":"Unicode with spaces"}),
    )
}
