#![allow(dead_code)]
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::{OsStr, OsString},
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitCode, ExitStatus, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug)]
pub struct UsageError(pub &'static str);

impl std::fmt::Display for UsageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for UsageError {}

pub struct Arguments {
    values: BTreeMap<String, OsString>,
    flags: BTreeSet<String>,
}

impl Arguments {
    pub fn read(options: &[&str], switches: &[&str]) -> Result<Option<Self>> {
        let mut args = env::args_os().skip(1);
        let mut values = BTreeMap::new();
        let mut flags = BTreeSet::new();
        while let Some(arg) = args.next() {
            let text = arg.to_str().ok_or("Invalid UTF-8 option name")?;
            if matches!(text, "-h" | "--help") {
                return Ok(None);
            }
            let (name, attached) = text
                .split_once('=')
                .map_or((text, None), |(key, value)| (key, Some(value)));
            if options.contains(&name) {
                let value = attached.map(OsString::from).or_else(|| {
                    args.next().filter(|value| {
                        !value.to_str().is_some_and(|value| value.starts_with("--"))
                    })
                });
                values.insert(name.to_owned(), value.ok_or("Missing option argument")?);
            } else if switches.contains(&name) && attached.is_none() {
                flags.insert(name.to_owned());
            } else {
                return Err(format!("Unrecognized argument {name}").into());
            }
        }
        Ok(Some(Self { values, flags }))
    }

    pub fn path(&self, key: &str) -> Result<PathBuf> {
        self.optional_path(key)
            .ok_or_else(|| UsageError("A required argument is missing").into())
    }

    pub fn optional_path(&self, key: &str) -> Option<PathBuf> {
        self.values.get(key).map(PathBuf::from)
    }

    pub fn flag(&self, key: &str) -> bool {
        self.flags.contains(key)
    }
}

pub fn cli(
    usage: &str,
    options: &[&str],
    switches: &[&str],
    run: impl FnOnce(Arguments) -> Result<()>,
) -> ExitCode {
    match Arguments::read(options, switches) {
        Ok(Some(args)) => match run(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) if error.is::<UsageError>() => {
                eprintln!("{usage}\n{error}");
                ExitCode::from(2)
            }
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        },
        Ok(None) => {
            println!("{usage}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{usage}\n{error}");
            ExitCode::from(2)
        }
    }
}

pub fn info(message: &str) {
    let time = jiff::Timestamp::now().to_string();
    eprintln!(
        "{}",
        json!({
            "timestamp":time.replace('Z', "+00:00"),
            "severity":"INFO",
            "component":"repository-tool",
            "message":message
        })
    );
}

pub fn hash_bytes(data: &[u8]) -> String {
    core_tooling::sha256_hex(data)
}

pub fn hash_file(path: &Path) -> io::Result<String> {
    core_tooling::sha256_file(path)
}

pub fn canonical_json(mut value: Value) -> Result<Vec<u8>> {
    value.sort_all_objects();
    let mut bytes = serde_json::to_vec_pretty(&value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn lexical_relative(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['\\', ':', '\0'])
        && name.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

pub fn git_tree(commit: &str, include: &[&str]) -> Result<BTreeSet<String>> {
    let output = checked(
        Command::new("git")
            .args(["ls-tree", "-rz", "--name-only", commit, "--"])
            .args(include),
        60,
    )?;
    let names = String::from_utf8(output.stdout)?;
    Ok(names.split_terminator('\0').map(str::to_owned).collect())
}

pub fn git_head() -> Result<String> {
    let output = checked(Command::new("git").args(["rev-parse", "HEAD"]), 10)?;
    let commit = String::from_utf8(output.stdout)?.trim().to_owned();
    if env::var_os("GITHUB_SHA").is_some_and(|value| value != commit.as_str()) {
        return Err("Package source does not match the workflow revision".into());
    }
    Ok(commit)
}

pub fn git_archive_zip(
    path: &Path,
    commit: &str,
    prefix: Option<&str>,
    include: &[&str],
    seconds: u64,
) -> Result<()> {
    let parent = path
        .parent()
        .ok_or("Missing source archive parent directory")?;
    let archive = tempfile::NamedTempFile::new_in(parent)?;
    let mut git = Command::new("git");
    git.args(["archive", "--format=zip"]);
    if let Some(prefix) = prefix {
        git.arg(format!("--prefix={prefix}"));
    }
    // Rust opens the destination because Git for Windows interprets verbatim
    // Windows paths passed as archive output arguments differently.
    let mut child = git
        .arg(commit)
        .arg("--")
        .args(include)
        .stdin(Stdio::null())
        .stdout(archive.reopen()?)
        .stderr(Stdio::piped())
        .spawn()?;
    drop(git);
    let stderr = child
        .stderr
        .take()
        .ok_or("Missing Git archive diagnostic pipe")?;
    let diagnostics = thread::spawn(move || read_pipe(stderr));
    let status = wait(&mut child, Duration::from_secs(seconds))?;
    let stderr = diagnostics
        .join()
        .map_err(|_| "Git archive diagnostic thread failed")??;
    check_status(OsStr::new("git"), status, &stderr)?;
    archive.persist_noclobber(path)?;
    Ok(())
}

pub fn checked(command: &mut Command, seconds: u64) -> Result<Output> {
    let output = capture(command, None, Duration::from_secs(seconds))?;
    check_status(command.get_program(), output.status, &output.stderr)?;
    Ok(output)
}

fn check_status(program: &OsStr, status: ExitStatus, stderr: &[u8]) -> Result<()> {
    if !status.success() {
        let message = String::from_utf8_lossy(stderr);
        return Err(format!(
            "{} failed with {status}: {}",
            program.to_string_lossy(),
            message.trim()
        )
        .into());
    }
    Ok(())
}

pub fn capture(command: &mut Command, input: Option<Vec<u8>>, timeout: Duration) -> Result<Output> {
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("Missing process output pipe")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("Missing process diagnostic pipe")?;
    let reader = thread::spawn(move || read_pipe(stdout));
    let diagnostics = thread::spawn(move || read_pipe(stderr));
    let writer = input.map(|input| {
        let mut stdin = child.stdin.take().expect("piped process input");
        thread::spawn(move || stdin.write_all(&input))
    });
    let status = wait(&mut child, timeout)?;
    if let Some(writer) = writer {
        writer.join().map_err(|_| "Process input thread failed")??;
    }
    Ok(Output {
        status,
        stdout: reader
            .join()
            .map_err(|_| "Process output thread failed")??,
        stderr: diagnostics
            .join()
            .map_err(|_| "Process diagnostic thread failed")??,
    })
}

fn wait(child: &mut Child, timeout: Duration) -> Result<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            outcome => {
                let _ = child.kill();
                let _ = child.wait();
                return match outcome {
                    Ok(None) => Err("Native tool exceeded its deadline".into()),
                    Err(error) => Err(error.into()),
                    Ok(Some(_)) => unreachable!(),
                };
            }
        }
    }
}

fn read_pipe(mut read: impl Read) -> io::Result<Vec<u8>> {
    const OUTPUT_LIMIT: u64 = 16 * 1024 * 1024;
    let mut bytes = Vec::new();
    (&mut read).take(OUTPUT_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > OUTPUT_LIMIT {
        return Err(io::Error::other(
            "Native tool output exceeded its size limit",
        ));
    }
    Ok(bytes)
}

pub fn write_new(path: &Path, data: &[u8]) -> Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .write_all(data)?;
    Ok(())
}
