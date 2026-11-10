//! Native containment for Windows 10+/Server 2016+.
//!
//! A child joins a kill-on-close job during CreateProcessW, before its first
//! instruction. There is no create-then-assign gap. The caller must already
//! hold admission and approve the executable; this is not engine selection,
//! manager authorization, detach, or recovery repair.
#![allow(unsafe_code)]

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io, mem,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    ptr,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_ALREADY_EXISTS, FILETIME, GetLastError, HANDLE, WAIT_FAILED, WAIT_OBJECT_0,
    },
    System::{
        JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
            QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
        },
        Threading::{
            CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
            DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetProcessTimes,
            InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
            PROC_THREAD_ATTRIBUTE_JOB_LIST, PROCESS_INFORMATION, STARTUPINFOEXW,
            UpdateProcThreadAttribute, WaitForSingleObject,
        },
    },
};

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "Invalid native process configuration",
    )
}

fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut text: Vec<u16> = value.encode_wide().collect();
    if text.contains(&0) {
        return Err(invalid());
    }
    text.push(0);
    Ok(text)
}

// Windows CRT argv quoting: preserve empty strings, spaces, literal quotes and
// terminal backslashes. No shell or PATH search participates in creation.
fn quoted(value: &OsStr) -> io::Result<Vec<u16>> {
    let text = wide(value)?;
    let mut result = vec![b'"' as u16];
    let mut slashes = 0;
    for &unit in &text[..text.len() - 1] {
        if unit == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        if unit == b'"' as u16 {
            result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2 + 1));
        } else {
            result.extend(std::iter::repeat_n(b'\\' as u16, slashes));
        }
        result.push(unit);
        slashes = 0;
    }
    result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    result.push(b'"' as u16);
    Ok(result)
}

fn environment(values: &BTreeMap<String, OsString>) -> io::Result<Vec<u16>> {
    let mut entries = BTreeMap::new();
    for (key, value) in values {
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err(invalid());
        }
        if entries.insert(key.to_ascii_uppercase(), value).is_some() {
            return Err(invalid());
        }
    }
    let mut block = Vec::new();
    for (key, value) in entries {
        block.extend(key.encode_utf16());
        block.push(b'=' as u16);
        block.extend(wide(value)?);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

struct AttributeList<'a> {
    storage: Vec<u128>,
    _jobs: &'a [HANDLE],
    initialized: bool,
}
impl<'a> AttributeList<'a> {
    fn new(jobs: &'a [HANDLE]) -> io::Result<Self> {
        let mut bytes = 0;
        // SAFETY: documented sizing call; no attribute list is initialized yet.
        unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut bytes) };
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut list = Self {
            storage: vec![0; bytes.div_ceil(mem::size_of::<u128>())],
            _jobs: jobs,
            initialized: false,
        };
        // SAFETY: 16-byte-aligned allocation covers the exact requested size and
        // is never resized; Windows initializes its opaque list in place.
        if unsafe { InitializeProcThreadAttributeList(list.raw(), 1, 0, &mut bytes) } == 0 {
            return Err(io::Error::last_os_error());
        }
        list.initialized = true;
        // SAFETY: initialized list; borrowed handle-array storage remains live
        // through this list's destructor and synchronous CreateProcessW.
        if unsafe {
            UpdateProcThreadAttribute(
                list.raw(),
                0,
                PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
                jobs.as_ptr().cast(),
                mem::size_of_val(jobs),
                ptr::null_mut(),
                ptr::null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(list)
    }
    fn raw(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_mut_ptr().cast()
    }
}
impl Drop for AttributeList<'_> {
    fn drop(&mut self) {
        // SAFETY: only successfully initialized lists reach this destructor;
        // allocation and borrowed job handles still exist for the entire call.
        if self.initialized {
            unsafe { DeleteProcThreadAttributeList(self.raw()) };
        }
    }
}

/// A process tree held by one non-inheritable kernel job handle. Dropping it
/// asks Windows to terminate all members. Admission must remain active until
/// `terminate_and_wait` proves that the job is empty; Drop alone is no proof.
pub struct OwnedProcessTree {
    job: OwnedHandle,
    process: OwnedHandle,
    pid: u32,
    birth_ticks: u64,
}

impl OwnedProcessTree {
    /// Spawn an explicitly approved native .exe using only the supplied child
    /// environment. Parent tokens/proxy secrets are not inherited implicitly.
    pub fn spawn(
        program: &Path,
        args: &[OsString],
        directory: &Path,
        child_environment: &BTreeMap<String, OsString>,
    ) -> io::Result<Self> {
        if !program.is_absolute()
            || !directory.is_absolute()
            || !program
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
        {
            return Err(invalid());
        }
        let application = wide(program.as_os_str())?;
        let current_directory = wide(directory.as_os_str())?;
        let environment = environment(child_environment)?;
        let mut command = quoted(program.as_os_str())?;
        for argument in args {
            command.push(b' ' as u16);
            command.extend(quoted(argument)?);
        }
        command.push(0);
        if command.len() > 32767 {
            return Err(invalid());
        }
        let name = format!(
            "Local\\Numera.Core.{}",
            super::admission::nonce().map_err(|_| io::Error::other("OS randomness unavailable"))?
        );
        let name = wide(OsStr::new(&name))?;
        // SAFETY: terminated immutable name, default token DACL, non-inherited
        // handle. No supplied pointer escapes the synchronous API call.
        let handle = unsafe { CreateJobObjectW(ptr::null(), name.as_ptr()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: read the creation result before another Win32 API can run.
        let collision = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        // SAFETY: successful CreateJobObjectW returned one owned kernel handle.
        let job = unsafe { OwnedHandle::from_raw_handle(handle) };
        if collision {
            return Err(io::Error::other("Job name collision"));
        }
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: owned live job and exact initialized POD buffer for its class.
        if unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                ptr::addr_of!(limits).cast(),
                mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let jobs = [handle];
        let mut attributes = AttributeList::new(&jobs)?;
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = mem::size_of::<STARTUPINFOEXW>() as u32;
        startup.lpAttributeList = attributes.raw();
        let mut information = PROCESS_INFORMATION::default();
        // SAFETY: all UTF-16 buffers are terminated and live. Windows may mutate
        // only the command-line/output buffers. Attribute storage and job array
        // live until after creation; inheritance is false. Application is an
        // absolute .exe, never a shell. The job assignment happens atomically.
        let ok = unsafe {
            CreateProcessW(
                application.as_ptr(),
                command.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
                environment.as_ptr().cast(),
                current_directory.as_ptr(),
                &startup.StartupInfo,
                &mut information,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful creation transfers two distinct CloseHandle-owned
        // handles. Dropping the thread handle does not terminate its thread.
        let process = unsafe { OwnedHandle::from_raw_handle(information.hProcess) };
        // SAFETY: hThread is the separately owned handle returned above.
        drop(unsafe { OwnedHandle::from_raw_handle(information.hThread) });
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: the owned process handle is held across this exact birth-time
        // query; output pointers are distinct initialized FILETIME structures.
        if unsafe {
            GetProcessTimes(
                process.as_raw_handle(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            job,
            process,
            pid: information.dwProcessId,
            birth_ticks: (u64::from(created.dwHighDateTime) << 32)
                | u64::from(created.dwLowDateTime),
        })
    }
    pub fn pid(&self) -> u32 {
        self.pid
    }
    pub fn birth_ticks(&self) -> u64 {
        self.birth_ticks
    }
    pub fn root_exited(&self) -> io::Result<bool> {
        // SAFETY: owned live process handle, nonblocking query only.
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_FAILED => Err(io::Error::last_os_error()),
            _ => Ok(false),
        }
    }
    pub fn active_processes(&self) -> io::Result<u32> {
        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        // SAFETY: live owned job and exact initialized output buffer/class.
        if unsafe {
            QueryInformationJobObject(
                self.job.as_raw_handle(),
                JobObjectBasicAccountingInformation,
                ptr::addr_of_mut!(info).cast(),
                mem::size_of_val(&info) as u32,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(info.ActiveProcesses)
    }
    pub fn terminate_and_wait(&self, timeout: Duration) -> io::Result<()> {
        let end = Instant::now().checked_add(timeout).ok_or_else(invalid)?;
        // SAFETY: this job contains only the tree assigned during our spawn.
        if unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        while self.active_processes()? != 0 {
            if Instant::now() >= end {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Owned descendants remain active",
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }
}
