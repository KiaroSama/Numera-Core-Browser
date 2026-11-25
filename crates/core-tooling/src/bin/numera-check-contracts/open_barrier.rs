// Development-only preload fixture compiled for the Linux x86-64 contract test.
#![deny(unsafe_op_in_unsafe_fn)]

use std::{
    arch::naked_asm,
    ffi::{CStr, c_char, c_int, c_uint, c_void},
};

const O_WRONLY: c_int = 1;
const O_CREAT: c_int = 0o100;
const O_EXCL: c_int = 0o200;
const O_CLOEXEC: c_int = 0o2000000;
const O_TMPFILE: c_int = 0o20200000;

unsafe extern "C" {
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn getenv(name: *const c_char) -> *const c_char;
    fn close(fd: c_int) -> c_int;
    fn access(path: *const c_char, mode: c_int) -> c_int;
    fn usleep(duration: c_uint) -> c_int;
    fn _exit(code: c_int) -> !;
}

// System V x86-64 passes the optional mode in EDX. Forward the original
// registers without defining a Rust C-variadic function on the stable compiler.
#[unsafe(naked)]
#[unsafe(no_mangle)]
unsafe extern "C" fn open64() -> ! {
    naked_asm!("jmp {}", sym barrier);
}

extern "C" fn barrier(path: *const c_char, flags: c_int, mode: c_uint) -> c_int {
    type NextOpen = unsafe extern "C" fn(*const c_char, c_int, ...) -> c_int;
    // SAFETY: The symbol is the next libc open64 definition. All strings
    // inspected here are valid C strings supplied by libc or the child fixture.
    unsafe {
        let symbol = dlsym((-1_isize) as *mut c_void, c"open64".as_ptr());
        if symbol.is_null() {
            _exit(120);
        }
        let open: NextOpen = std::mem::transmute(symbol);
        let target = getenv(c"NUMERA_TEST_OPEN_TARGET".as_ptr());
        let ready = getenv(c"NUMERA_TEST_OPEN_READY".as_ptr());
        let resume = getenv(c"NUMERA_TEST_OPEN_RESUME".as_ptr());
        if !target.is_null()
            && !ready.is_null()
            && !resume.is_null()
            && !path.is_null()
            && CStr::from_ptr(path) == CStr::from_ptr(target)
        {
            let signal = open(
                ready,
                O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC,
                0o600 as c_uint,
            );
            if signal < 0 {
                _exit(121);
            }
            close(signal);
            for attempt in 0..=10_000 {
                if access(resume, 0) == 0 {
                    break;
                }
                if attempt == 10_000 {
                    _exit(122);
                }
                usleep(1000);
            }
        }
        if flags & O_CREAT != 0 || flags & O_TMPFILE == O_TMPFILE {
            open(path, flags, mode)
        } else {
            open(path, flags)
        }
    }
}
