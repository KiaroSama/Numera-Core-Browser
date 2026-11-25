use super::{Disk, Memory, sum_tree};
use std::{
    io, mem,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
};
use windows_sys::Win32::{
    Foundation::INVALID_HANDLE_VALUE,
    Storage::FileSystem::GetDiskFreeSpaceExW,
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
        Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    },
};

pub fn disk(path: &Path) -> io::Result<Disk> {
    let path: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0;
    let mut total = 0;
    // SAFETY: live NUL-terminated path and initialized writable counters.
    let valid =
        unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) };
    if valid == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Disk { free, total })
}

pub fn memory() -> io::Result<Memory> {
    let mut status = MEMORYSTATUSEX {
        dwLength: mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..MEMORYSTATUSEX::default()
    };
    // SAFETY: status has the Win32-defined structure layout and size.
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Memory {
        total: status.ullTotalPhys,
        available: status.ullAvailPhys,
    })
}

fn resident_bytes(pid: u32) -> u64 {
    // SAFETY: no inherited process handle and only query access.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return 0;
    }
    // SAFETY: OpenProcess returned a fresh CloseHandle-owned handle.
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..PROCESS_MEMORY_COUNTERS::default()
    };
    // SAFETY: the process handle and writable Win32-sized counters outlive the call.
    if unsafe { GetProcessMemoryInfo(handle.as_raw_handle(), &mut counters, counters.cb) } == 0 {
        return 0;
    }
    counters.WorkingSetSize as u64
}

pub fn process_tree(pid: u32) -> (u64, usize) {
    // SAFETY: requests a read-only system snapshot, with a CloseHandle result.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return (0, 0);
    }
    // SAFETY: successful CreateToolhelp32Snapshot returned an owned handle.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let mut item = PROCESSENTRY32W {
        dwSize: mem::size_of::<PROCESSENTRY32W>() as u32,
        ..PROCESSENTRY32W::default()
    };
    // SAFETY: valid snapshot handle and correctly sized writable entry.
    let mut available = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut item) };
    let mut rows = Vec::new();
    while available != 0 {
        rows.push((
            item.th32ProcessID,
            item.th32ParentProcessID,
            resident_bytes(item.th32ProcessID),
        ));
        // SAFETY: same live snapshot and writable Win32 entry as the first call.
        available = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut item) };
    }
    sum_tree(pid, &rows)
}
