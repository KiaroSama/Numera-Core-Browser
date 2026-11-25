use std::{collections::BTreeSet, process::Child, time::Duration};

use super::wait;

#[derive(Clone)]
pub(in crate::fixtures) struct Process {
    pub pid: u32,
    pub parent: u32,
    started: u64,
    pub name: String,
}

pub(in crate::fixtures) fn descendants(pid: u32) -> Vec<Process> {
    let mut pending = platform::snapshot();
    let mut parents = BTreeSet::from([pid]);
    let mut found = Vec::new();
    loop {
        let count = found.len();
        pending.retain(|process| {
            if parents.contains(&process.parent) && parents.insert(process.pid) {
                found.push(process.clone());
                false
            } else {
                true
            }
        });
        if found.len() == count {
            return found;
        }
    }
}

pub(in crate::fixtures) fn exited(process: &Process) -> bool {
    platform::exited(process)
}

#[cfg(windows)]
mod windows_arguments;
#[cfg(windows)]
pub(in crate::fixtures) use platform::windows_arguments;

pub(in crate::fixtures) fn stop_tree(child: &mut Child) {
    if !matches!(child.try_wait(), Ok(None)) {
        return;
    }
    let cohort = descendants(child.id());
    for process in cohort.iter().rev() {
        platform::terminate(process);
    }
    let _ = child.kill();
    let _ = wait(child, Duration::from_secs(5));
}

#[cfg(target_os = "linux")]
mod platform {
    use super::Process;
    use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};
    use std::fs;

    fn process(pid: u32) -> Option<Process> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let (_, fields) = stat.rsplit_once(')')?;
        let fields = fields.split_whitespace().collect::<Vec<_>>();
        let state = *fields.first()?;
        if matches!(state, "Z" | "X") {
            return None;
        }
        let name = stat.split_once('(')?.1.rsplit_once(')')?.0.to_owned();
        Some(Process {
            pid,
            parent: fields.get(1)?.parse().ok()?,
            started: fields.get(19)?.parse().ok()?,
            name,
        })
    }

    pub(super) fn snapshot() -> Vec<Process> {
        fs::read_dir("/proc")
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let pid = entry.ok()?.file_name().to_str()?.parse::<u32>().ok()?;
                process(pid)
            })
            .collect()
    }

    pub(super) fn exited(known: &Process) -> bool {
        process(known.pid).is_none_or(|current| current.started != known.started)
    }

    pub(super) fn terminate(known: &Process) {
        if let Some(pid) = i32::try_from(known.pid).ok().and_then(Pid::from_raw)
            && let Ok(fd) = pidfd_open(pid, PidfdFlags::empty())
            && !exited(known)
        {
            let _ = pidfd_send_signal(&fd, Signal::KILL);
        }
    }
}

#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "Windows process cleanup requires native handles and identity checks"
)]
mod platform {
    use super::Process;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::{FILETIME, INVALID_HANDLE_VALUE, WAIT_OBJECT_0},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS,
            },
            Threading::{
                GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
                PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
            },
        },
    };

    fn acquire(pid: u32, access: u32) -> Option<OwnedHandle> {
        // SAFETY: numeric access flags and PID, no data pointers.
        let raw = unsafe { OpenProcess(access, 0, pid) };
        if raw.is_null() {
            return None;
        }
        // SAFETY: the successful call returned a handle owned by this caller.
        Some(unsafe { OwnedHandle::from_raw_handle(raw) })
    }

    fn identity(handle: &OwnedHandle) -> Option<u64> {
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: the handle and four correctly sized write buffers are live.
        let result = unsafe {
            GetProcessTimes(
                handle.as_raw_handle(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        };
        (result != 0)
            .then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
    }

    pub(in crate::fixtures) fn windows_arguments(known: &Process) -> Option<Vec<String>> {
        let handle = acquire(known.pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
        if identity(&handle) != Some(known.started) {
            return None;
        }
        super::windows_arguments::read(&handle)
    }

    pub(super) fn snapshot() -> Vec<Process> {
        // SAFETY: this documented snapshot uses no input pointers.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Vec::new();
        }
        // SAFETY: a successful snapshot returns one owned handle.
        let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut result = Vec::new();
        // SAFETY: the snapshot and initialized output record remain live throughout the calls.
        unsafe {
            let mut available = Process32FirstW(snapshot.as_raw_handle(), &mut entry);
            while available != 0 {
                let pid = entry.th32ProcessID;
                if let Some(started) = acquire(pid, PROCESS_QUERY_LIMITED_INFORMATION)
                    .as_ref()
                    .and_then(identity)
                {
                    let end = entry
                        .szExeFile
                        .iter()
                        .position(|unit| *unit == 0)
                        .unwrap_or(entry.szExeFile.len());
                    result.push(Process {
                        pid,
                        parent: entry.th32ParentProcessID,
                        started,
                        name: String::from_utf16_lossy(&entry.szExeFile[..end]),
                    });
                }
                available = Process32NextW(snapshot.as_raw_handle(), &mut entry);
            }
        }
        result
    }

    pub(super) fn exited(known: &Process) -> bool {
        acquire(
            known.pid,
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
        )
        .is_none_or(|handle| {
            // SAFETY: the owned process handle has synchronization access.
            identity(&handle) != Some(known.started)
                || unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) == WAIT_OBJECT_0 }
        })
    }

    pub(super) fn terminate(known: &Process) {
        if let Some(handle) = acquire(
            known.pid,
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
        ) && identity(&handle) == Some(known.started)
        {
            // SAFETY: creation time matches this owned descendant and this handle authorizes its termination.
            unsafe {
                TerminateProcess(handle.as_raw_handle(), 1);
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    use super::Process;

    pub(super) fn snapshot() -> Vec<Process> {
        Vec::new()
    }
    pub(super) fn exited(_: &Process) -> bool {
        true
    }
    pub(super) fn terminate(_: &Process) {}
}
