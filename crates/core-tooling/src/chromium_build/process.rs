//! Bounded native commands and resource measurements used on hosted workers.
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs::File,
    io::{self, Read, Seek},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::fs;

pub type Environment = BTreeMap<OsString, OsString>;

pub fn build_environment() -> Environment {
    std::env::vars_os()
        .filter(|(key, _)| {
            let name = key.to_string_lossy().to_ascii_uppercase();
            !name.starts_with("CCACHE_")
                && !name.starts_with("SCCACHE_")
                && !matches!(
                    name.as_str(),
                    "GH_TOKEN"
                        | "GITHUB_TOKEN"
                        | "ACTIONS_RUNTIME_TOKEN"
                        | "ACTIONS_ID_TOKEN_REQUEST_TOKEN"
                )
        })
        .collect()
}

pub fn command(program: impl AsRef<std::ffi::OsStr>, environment: &Environment) -> Command {
    let mut command = Command::new(program);
    command.env_clear().envs(environment);
    command
}

pub fn binary_sibling(name: &str) -> io::Result<PathBuf> {
    let sibling = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    Ok(std::env::current_exe()?.with_file_name(sibling))
}

pub struct ChildWatch(Child);

impl ChildWatch {
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        Ok(Self(command.spawn()?))
    }

    pub fn pid(&self) -> u32 {
        self.0.id()
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.0.try_wait()
    }

    pub fn wait(&mut self, timeout: Duration) -> io::Result<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Owned native command exceeded its time budget",
                ));
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for ChildWatch {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(Some(_))) {
            return;
        }
        #[cfg(unix)]
        if let Some(pid) = rustix::process::Pid::from_raw(self.0.id() as i32) {
            use rustix::process::{Signal, kill_process_group};
            let _ = kill_process_group(pid, Signal::TERM);
            thread::sleep(Duration::from_millis(200));
            let _ = kill_process_group(pid, Signal::KILL);
        }
        #[cfg(windows)]
        {
            let _ = Command::new("taskkill.exe")
                .args(["/T", "/F", "/PID", &self.0.id().to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn stream_log(command: &mut Command, path: &Path) -> io::Result<ChildWatch> {
    let file = File::create(path)?;
    command
        .stdout(file.try_clone()?)
        .stderr(file)
        .stdin(Stdio::null());
    ChildWatch::spawn(command)
}

pub struct Captured {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub fn capture(command: &mut Command, timeout: Duration, maximum: u64) -> io::Result<Captured> {
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    command
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?)
        .stdin(Stdio::null());
    let status = ChildWatch::spawn(command)?.wait(timeout)?;
    fn read(file: &mut File, maximum: u64) -> io::Result<Vec<u8>> {
        if file.metadata()?.len() > maximum {
            return Err(io::Error::other("Native command output exceeds its limit"));
        }
        file.rewind()?;
        let mut bytes = Vec::new();
        file.take(maximum + 1).read_to_end(&mut bytes)?;
        Ok(bytes)
    }
    Ok(Captured {
        status,
        stdout: read(&mut stdout, maximum)?,
        stderr: read(&mut stderr, maximum)?,
    })
}

pub fn stdout(command: &mut Command, timeout: Duration, maximum: u64) -> io::Result<Vec<u8>> {
    let result = capture(command, timeout, maximum)?;
    if result.status.success() {
        Ok(result.stdout)
    } else {
        Err(io::Error::other(format!(
            "Bounded native command exited unsuccessfully ({})",
            result.status
        )))
    }
}

pub fn prepend_path(environment: &mut Environment, paths: &[&Path]) -> io::Result<()> {
    let paths = paths.iter().map(|path| (*path).to_owned()).chain(
        environment
            .get(&OsString::from("PATH"))
            .into_iter()
            .flat_map(std::env::split_paths),
    );
    let path = std::env::join_paths(paths).map_err(io::Error::other)?;
    environment.insert("PATH".into(), path);
    Ok(())
}

pub struct Disk {
    pub total: u64,
    pub free: u64,
}

pub struct Memory {
    pub total: u64,
    pub available: u64,
}

pub fn cpu_count() -> usize {
    thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

fn sum_tree(pid: u32, rows: &[(u32, u32, u64)]) -> (u64, usize) {
    let mut pids = BTreeSet::new();
    let mut queue = vec![pid];
    let mut rss = 0_u64;
    while let Some(next) = queue.pop() {
        if pids.insert(next) {
            for (process, parent, bytes) in rows {
                if *process == next {
                    rss = rss.saturating_add(*bytes);
                } else if *parent == next {
                    queue.push(*process);
                }
            }
        }
    }
    (rss, pids.len())
}

#[cfg(unix)]
pub fn disk(path: &Path) -> io::Result<Disk> {
    let capacity = rustix::fs::statvfs(path)?;
    Ok(Disk {
        total: capacity.f_blocks.saturating_mul(capacity.f_frsize),
        free: capacity.f_bavail.saturating_mul(capacity.f_frsize),
    })
}

#[cfg(unix)]
pub fn memory() -> io::Result<Memory> {
    let text = fs::read_to_string("/proc/meminfo")?;
    let bytes = |name| {
        linux_status(&text, name)
            .map(|kib| kib.saturating_mul(1024))
            .ok_or_else(|| io::Error::other("Host memory metrics unavailable"))
    };
    Ok(Memory {
        total: bytes("MemTotal")?,
        available: bytes("MemAvailable")?,
    })
}

#[cfg(unix)]
fn linux_status(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let (name, rest) = line.split_once(':')?;
        (name == key)
            .then(|| rest.split_whitespace().next()?.parse().ok())
            .flatten()
    })
}

#[cfg(unix)]
pub fn process_tree(pid: u32) -> (u64, usize) {
    let Ok(entries) = fs::read_dir("/proc") else {
        return (0, 0);
    };
    let rows: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let proc = entry.file_name().to_str()?.parse().ok()?;
            let status = fs::read_to_string(entry.path().join("status")).ok()?;
            let parent = u32::try_from(linux_status(&status, "PPid")?).ok()?;
            let rss = linux_status(&status, "VmRSS")
                .unwrap_or(0)
                .saturating_mul(1024);
            Some((proc, parent, rss))
        })
        .collect();
    sum_tree(pid, &rows)
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;
#[cfg(windows)]
pub use windows::{disk, memory, process_tree};
