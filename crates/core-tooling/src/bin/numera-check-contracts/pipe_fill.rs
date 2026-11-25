use crate::common::Result;
use std::io::{self, PipeWriter, Write};

pub(super) fn saturate(writer: &mut PipeWriter) -> Result<()> {
    let flags = nonblocking(writer)?;
    let result = fill(writer);
    result.and(blocking(writer, flags))
}

fn fill(writer: &mut PipeWriter) -> Result<()> {
    const BLOCK: [u8; 4096] = [b'x'; 4096];
    const LIMIT: usize = 8 * 1024 * 1024;
    let mut bytes = &BLOCK[..];
    let mut filled = 0;
    loop {
        let written = match writer.write(bytes) {
            Ok(written) => written,
            Err(error) if full(&error) => 0,
            Err(error) => return Err(error.into()),
        };
        if written == 0 {
            if bytes.len() == 1 {
                return if filled > 0 {
                    Ok(())
                } else {
                    Err("Diagnostic pipe rejected its first byte".into())
                };
            }
            bytes = &BLOCK[..1];
        } else {
            filled += written;
            if filled > LIMIT {
                return Err("Stderr pipe did not reach bounded saturation".into());
            }
        }
    }
}

fn full(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    if error.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_NO_DATA as i32) {
        return true;
    }
    false
}

#[cfg(unix)]
fn nonblocking(writer: &PipeWriter) -> Result<rustix::fs::OFlags> {
    use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
    let flags = fcntl_getfl(writer)?;
    fcntl_setfl(writer, flags | OFlags::NONBLOCK)?;
    Ok(flags)
}

#[cfg(unix)]
fn blocking(writer: &PipeWriter, flags: rustix::fs::OFlags) -> Result<()> {
    Ok(rustix::fs::fcntl_setfl(writer, flags)?)
}

#[cfg(windows)]
struct BlockingMode;

#[cfg(windows)]
fn nonblocking(writer: &PipeWriter) -> Result<BlockingMode> {
    set_mode(writer, windows_sys::Win32::System::Pipes::PIPE_NOWAIT)?;
    Ok(BlockingMode)
}

#[cfg(windows)]
fn blocking(writer: &PipeWriter, _: BlockingMode) -> Result<()> {
    set_mode(writer, windows_sys::Win32::System::Pipes::PIPE_WAIT)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn set_mode(writer: &PipeWriter, mode: u32) -> Result<()> {
    use std::{os::windows::io::AsRawHandle, ptr};
    use windows_sys::Win32::System::Pipes::SetNamedPipeHandleState;

    // SAFETY: The live pipe writer owns this handle. The mode pointer
    // is valid for the call and the optional timeout pointers are null.
    if unsafe { SetNamedPipeHandleState(writer.as_raw_handle(), &mode, ptr::null(), ptr::null()) }
        == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}
