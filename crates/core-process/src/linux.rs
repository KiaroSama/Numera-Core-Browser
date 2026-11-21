//! Process ownership for a dedicated, single-threaded Linux session worker.
//!
//! The worker's only child processes must belong to its session. The kernel
//! subreaper reparents orphaned descendants even after a process group changes.
use core_contracts::{CoreError, ErrorCode};
use nix::sys::prctl::{get_child_subreaper, set_child_subreaper};
use nix::unistd::{getpid, gettid};
use rustix::{
    fd::{AsFd, OwnedFd},
    io::Errno,
    process::{
        Pid, PidfdFlags, Signal, WaitId, WaitIdOptions, pidfd_open, pidfd_send_signal, waitid,
    },
};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    marker::PhantomData,
    path::Path,
    process::{Command, Stdio},
    rc::Rc,
    time::{Duration, Instant},
};

const WAIT_CHILDREN: WaitIdOptions = WaitIdOptions::EXITED
    .union(WaitIdOptions::NOHANG)
    .union(WaitIdOptions::from_bits_retain(nix::libc::__WALL as u32));
const OBSERVE_EXIT: WaitIdOptions = WAIT_CHILDREN.union(WaitIdOptions::NOWAIT);

fn recovery(message: &'static str) -> CoreError {
    CoreError::new(ErrorCode::RecoveryRequired, message)
}

fn status_field<'a>(status: &'a [u8], field: &[u8]) -> Option<&'a str> {
    status
        .split(|byte| *byte == b'\n')
        .find_map(|line| line.strip_prefix(field))
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .map(str::trim)
}

fn default_child_signal() -> Result<bool, CoreError> {
    let status = fs::read("/proc/self/status")
        .map_err(|_| recovery("Cannot inspect Linux worker signal disposition"))?;
    let child_signal = 1u64 << (nix::libc::SIGCHLD - 1);
    // A fresh exec clears signal flags and caught handlers. Ignored signals
    // can survive exec and would let the kernel reap children before us.
    Ok([b"SigIgn:".as_slice(), b"SigCgt:"]
        .into_iter()
        .all(|field| {
            status_field(&status, field)
                .and_then(|value| u64::from_str_radix(value, 16).ok())
                .is_some_and(|mask| mask & child_signal == 0)
        }))
}

fn stop_children(deadline: Instant) -> Result<(), CoreError> {
    let parent = std::process::id().to_string();
    loop {
        let reap_again = match waitid(WaitId::All, WAIT_CHILDREN) {
            Err(Errno::CHILD) => return Ok(()),
            Err(Errno::INTR) | Ok(Some(_)) => true,
            Ok(None) => false,
            Err(_) => return Err(recovery("Cannot inspect Linux session children")),
        };
        if Instant::now() >= deadline {
            return Err(recovery(
                "Linux session children did not stop within the deadline",
            ));
        }
        if reap_again {
            continue;
        }
        let processes = fs::read_dir("/proc")
            .map_err(|_| recovery("Cannot enumerate Linux process ownership"))?;
        for entry in processes.flatten() {
            if Instant::now() >= deadline {
                break;
            }
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<i32>().ok())
                .filter(|id| *id > 0)
                .and_then(Pid::from_raw)
            else {
                continue;
            };
            let Ok(status) = fs::read(entry.path().join("status")) else {
                continue;
            };
            if status_field(&status, b"PPid:") != Some(parent.as_str()) {
                continue;
            }
            let fd = match pidfd_open(pid, PidfdFlags::empty()) {
                Ok(fd) => fd,
                Err(Errno::SRCH | Errno::INTR) => continue,
                Err(_) => return Err(recovery("Cannot open a Linux session child handle")),
            };
            // A /proc snapshot can contain an exited PID. Ask the kernel to
            // confirm that this exact handle still belongs to our child.
            match waitid(WaitId::PidFd(fd.as_fd()), OBSERVE_EXIT) {
                Ok(None) => match pidfd_send_signal(&fd, Signal::KILL) {
                    Ok(()) | Err(Errno::SRCH | Errno::INTR) => {}
                    Err(_) => return Err(recovery("Cannot stop a Linux session child")),
                },
                Ok(Some(_)) | Err(Errno::CHILD | Errno::INTR) => {}
                Err(_) => return Err(recovery("Cannot confirm Linux session child ownership")),
            }
        }
        // The kernel can omit children from a changing /proc snapshot.
        // Only ECHILD, after reaping exited children, proves an empty tree.
        std::thread::sleep(
            Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

pub struct OwnedProcessTree {
    root: Option<OwnedFd>,
    worker: PhantomData<Rc<()>>,
}

impl OwnedProcessTree {
    /// Start one session in a freshly executed worker. The worker must retain
    /// exclusive child ownership and keep the default child-exit disposition.
    pub fn spawn(
        executable: &Path,
        arguments: &[OsString],
        directory: &Path,
        environment: &BTreeMap<String, OsString>,
    ) -> Result<Self, CoreError> {
        if !executable.is_absolute() || !directory.is_absolute() {
            return Err(CoreError::invalid("Linux launch requires absolute paths"));
        }
        let mut threads = fs::read_dir("/proc/self/task")
            .map_err(|_| recovery("Cannot inspect Linux worker threads"))?;
        let dedicated = getpid() == gettid()
            && matches!(threads.next(), Some(Ok(_)))
            && threads.next().is_none()
            && matches!(waitid(WaitId::All, OBSERVE_EXIT), Err(Errno::CHILD))
            && matches!(get_child_subreaper(), Ok(false));
        if !dedicated || !default_child_signal()? {
            return Err(CoreError::new(
                ErrorCode::OwnershipConflict,
                "Linux launch requires a dedicated session worker",
            ));
        }
        let current = pidfd_open(rustix::process::getpid(), PidfdFlags::empty()).map_err(|_| {
            CoreError::new(
                ErrorCode::LaunchFailure,
                "Linux process handles are unavailable",
            )
        })?;
        if !matches!(
            waitid(WaitId::PidFd(current.as_fd()), OBSERVE_EXIT),
            Err(Errno::CHILD)
        ) {
            return Err(CoreError::new(
                ErrorCode::LaunchFailure,
                "Linux process-handle waits are unavailable",
            ));
        }
        set_child_subreaper(true)
            .map_err(|_| recovery("Cannot opt into Linux descendant adoption"))?;
        let started = Command::new(executable)
            .args(arguments)
            .current_dir(directory)
            .env_clear()
            .envs(environment)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        // All waits are through waitid, including the root's eventual reap.
        #[allow(clippy::zombie_processes)]
        let child = match started {
            Ok(child) => child,
            Err(_) => {
                set_child_subreaper(false)
                    .map_err(|_| recovery("Cannot reset Linux worker after launch failure"))?;
                return Err(CoreError::new(
                    ErrorCode::LaunchFailure,
                    "Could not start the Linux session process",
                ));
            }
        };
        let root = pidfd_open(Pid::from_child(&child), PidfdFlags::empty())
            .and_then(|fd| waitid(WaitId::PidFd(fd.as_fd()), OBSERVE_EXIT).map(|_| fd));
        match root {
            Ok(root) => Ok(Self {
                root: Some(root),
                worker: PhantomData,
            }),
            Err(_) => {
                stop_children(Instant::now() + Duration::from_secs(2))?;
                set_child_subreaper(false)
                    .map_err(|_| recovery("Cannot reset Linux worker after handle failure"))?;
                Err(CoreError::new(
                    ErrorCode::LaunchFailure,
                    "Linux process-handle support is unavailable",
                ))
            }
        }
    }

    pub fn root_exited(&self) -> Result<bool, CoreError> {
        let Some(root) = &self.root else {
            return Ok(true);
        };
        waitid(WaitId::PidFd(root.as_fd()), OBSERVE_EXIT)
            .map(|status| status.is_some())
            .map_err(|_| recovery("Cannot observe the Linux session root"))
    }

    pub fn terminate_and_wait(&mut self, timeout: Duration) -> Result<(), CoreError> {
        if self.root.is_some() {
            let deadline = Instant::now()
                .checked_add(timeout)
                .ok_or_else(|| CoreError::invalid("Invalid Linux session shutdown deadline"))?;
            stop_children(deadline)?;
            set_child_subreaper(false)
                .map_err(|_| recovery("Cannot reset the Linux session worker"))?;
            self.root = None;
        }
        Ok(())
    }
}

impl Drop for OwnedProcessTree {
    fn drop(&mut self) {
        // Claim release requires a successful explicit terminate_and_wait.
        let _ = self.terminate_and_wait(Duration::from_secs(2));
    }
}
