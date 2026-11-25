//! Read diagnostic startup arguments from a held Windows child handle.
#![expect(
    unsafe_code,
    reason = "Windows reports process arguments through a native buffer"
)]
use std::{
    ffi::c_void,
    os::windows::io::{AsRawHandle, OwnedHandle},
};
use windows_sys::Win32::Foundation::{HANDLE, LocalFree};

#[repr(C)]
struct UnicodeString {
    length: u16,
    capacity: u16,
    buffer: *const u16,
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(
        process: HANDLE,
        class: i32,
        information: *mut c_void,
        length: u32,
        written: *mut u32,
    ) -> i32;
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn CommandLineToArgvW(command: *const u16, argc: *mut i32) -> *mut *mut u16;
}

pub(super) fn read(handle: &OwnedHandle) -> Option<Vec<String>> {
    const PROCESS_COMMAND_LINE_INFORMATION: i32 = 60;
    const CAPACITY: usize = 128 * 1024;
    let mut storage = vec![0_usize; CAPACITY / std::mem::size_of::<usize>()];
    let mut written = 0;
    // SAFETY: the live process handle and aligned, size-checked output buffers
    // remain valid throughout this synchronous diagnostic query.
    let status = unsafe {
        NtQueryInformationProcess(
            handle.as_raw_handle(),
            PROCESS_COMMAND_LINE_INFORMATION,
            storage.as_mut_ptr().cast(),
            CAPACITY as u32,
            &mut written,
        )
    };
    if status < 0
        || !(std::mem::size_of::<UnicodeString>()..=CAPACITY).contains(&(written as usize))
    {
        return None;
    }
    // SAFETY: the successful kernel query supplied an aligned record.
    let command = unsafe { &*storage.as_ptr().cast::<UnicodeString>() };
    let length = usize::from(command.length);
    let offset = (command.buffer as usize).checked_sub(storage.as_ptr() as usize)?;
    if length == 0
        || command.length > command.capacity
        || !length.is_multiple_of(2)
        || !offset.is_multiple_of(2)
        || offset.checked_add(length)? > written as usize
    {
        return None;
    }
    // SAFETY: the native pointer and UTF-16 length were checked against the
    // output buffer. Derive the pointer from that allocation.
    let mut wide = unsafe {
        let data = storage.as_ptr().cast::<u8>().add(offset).cast::<u16>();
        std::slice::from_raw_parts(data, length / 2).to_vec()
    };
    wide.push(0);
    let mut argc = 0;
    // SAFETY: wide is terminated. The shell parser allocates the returned argv.
    let argv = unsafe { CommandLineToArgvW(wide.as_ptr(), &mut argc) };
    if argv.is_null() {
        return None;
    }
    // SAFETY: the system parser returned argc terminated argument strings.
    // Free the argv allocation after copying.
    unsafe {
        let result = (1..argc)
            .map(|index| {
                let begin = *argv.add(index as usize);
                let length = (0..wide.len())
                    .take_while(|offset| *begin.add(*offset) != 0)
                    .count();
                String::from_utf16_lossy(std::slice::from_raw_parts(begin, length))
            })
            .collect();
        LocalFree(argv.cast());
        Some(result)
    }
}
