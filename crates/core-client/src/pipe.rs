use crate::TransportError;
use core_contracts::MAX_FRAME;
use std::{
    env,
    fs::File,
    io,
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
#[path = "pipe/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "pipe/windows.rs"]
mod platform;

const POLL: Duration = Duration::from_millis(5);
const CLEANUP_BUDGET: Duration = Duration::from_secs(8);
const SERVICE_ENVIRONMENT: &[&str] = &[
    "HOME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "XDG_RUNTIME_DIR",
    "XDG_SESSION_TYPE",
    "XDG_CURRENT_DESKTOP",
    "DBUS_SESSION_BUS_ADDRESS",
    "SystemRoot",
    "WINDIR",
    "SYSTEMDRIVE",
    "USERPROFILE",
    "LOCALAPPDATA",
    "APPDATA",
    "PROGRAMDATA",
];

pub(crate) fn remaining(deadline: Instant) -> Result<Duration, TransportError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or(TransportError::Deadline)
}

pub(crate) struct Pipes {
    write_end: Option<File>,
    read_end: Option<File>,
    child: Option<Child>,
    reaped: Option<Receiver<io::Result<ExitStatus>>>,
    status: Option<ExitStatus>,
}

impl Pipes {
    pub(crate) fn open(
        binary: &Path,
        store: &Path,
        data_root: &Path,
    ) -> Result<Self, TransportError> {
        let mut command = Command::new(binary);
        command
            .arg("serve")
            .arg(store)
            .arg(data_root)
            .stderr(Stdio::null())
            .env_clear()
            .envs(
                SERVICE_ENVIRONMENT
                    .iter()
                    .filter_map(|name| env::var_os(name).map(|value| (*name, value))),
            );
        let (child, write_end, read_end) = platform::spawn(&mut command)?;
        Ok(Self {
            write_end: Some(write_end),
            read_end: Some(read_end),
            child: Some(child),
            reaped: None,
            status: None,
        })
    }

    pub(crate) fn is_open(&self) -> bool {
        self.write_end.is_some()
    }

    pub(crate) fn exchange(
        &mut self,
        mut request: &[u8],
        deadline: Instant,
    ) -> Result<Vec<u8>, TransportError> {
        let input = self.write_end.as_mut().ok_or(TransportError::Closed)?;
        while !request.is_empty() {
            let wait = remaining(deadline)?.min(POLL);
            match platform::write(input, request) {
                Ok(0) => thread::sleep(wait),
                Ok(count) => request = &request[count..],
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(wait),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => (),
                Err(_) => return Err(TransportError::Pipe),
            }
        }

        let output = self.read_end.as_mut().ok_or(TransportError::Closed)?;
        let mut frame = Vec::new();
        let mut buffer = [0u8; 8192];
        while frame.len() < MAX_FRAME {
            let wait = remaining(deadline)?.min(POLL);
            let limit = buffer.len().min(MAX_FRAME - frame.len());
            match platform::read(output, &mut buffer[..limit]) {
                Ok(0) => return Err(TransportError::Closed),
                Ok(count) => {
                    let latest = &buffer[..count];
                    if let Some(lf) = latest.iter().position(|byte| *byte == b'\n') {
                        if lf + 1 != count {
                            return Err(TransportError::InvalidResponse);
                        }
                        frame.extend_from_slice(&latest[..lf]);
                        return Ok(frame);
                    }
                    frame.extend_from_slice(latest);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(wait),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => (),
                Err(_) => return Err(TransportError::Pipe),
            }
        }
        Err(TransportError::FrameLimit)
    }

    pub(crate) fn shutdown(&mut self) {
        // The service owns worker cleanup. Close stdin before releasing stdout.
        drop(self.write_end.take());
        drop(self.read_end.take());
        let Some(child) = self.child.take() else {
            return;
        };
        let (send_child, receive_child) = mpsc::sync_channel::<Child>(1);
        let (send_result, receive_result) = mpsc::sync_channel(1);
        let reaper = thread::Builder::new()
            .name("numera-core-reap".into())
            .spawn(move || {
                if let Ok(mut child) = receive_child.recv() {
                    let _ = send_result.send(child.wait());
                }
            });
        if reaper.is_err() {
            self.child = Some(child);
        } else if let Err(mpsc::SendError(child)) = send_child.send(child) {
            self.child = Some(child);
        } else {
            self.reaped = Some(receive_result);
        }
    }

    pub(crate) fn close(&mut self) -> Result<ExitStatus, TransportError> {
        self.shutdown();
        if let Some(status) = self.status {
            return Ok(status);
        }
        let status = if let Some(reaped) = &self.reaped {
            reaped
                .recv_timeout(CLEANUP_BUDGET)
                .map_err(|error| match error {
                    mpsc::RecvTimeoutError::Timeout => TransportError::CleanupTimeout,
                    mpsc::RecvTimeoutError::Disconnected => TransportError::ExitStatusUnavailable,
                })?
                .map_err(|_| TransportError::ExitStatusUnavailable)?
        } else {
            let child = self
                .child
                .as_mut()
                .ok_or(TransportError::ExitStatusUnavailable)?;
            let deadline = Instant::now() + CLEANUP_BUDGET;
            loop {
                if let Some(status) = child
                    .try_wait()
                    .map_err(|_| TransportError::ExitStatusUnavailable)?
                {
                    break status;
                }
                let wait = remaining(deadline).map_err(|_| TransportError::CleanupTimeout)?;
                thread::sleep(wait.min(POLL));
            }
        };
        self.status = Some(status);
        self.child = None;
        self.reaped = None;
        Ok(status)
    }
}

impl Drop for Pipes {
    fn drop(&mut self) {
        self.shutdown();
    }
}
