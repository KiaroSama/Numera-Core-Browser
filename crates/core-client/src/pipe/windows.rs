#![allow(unsafe_code)]

use crate::TransportError;
use std::{
    fs::File,
    io,
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    process::{Child, Command, Stdio},
    ptr,
};
use windows_sys::Win32::{
    Foundation::ERROR_NO_DATA,
    System::{
        Pipes::{CreatePipe, PIPE_NOWAIT, SetNamedPipeHandleState},
        Threading::CREATE_NEW_PROCESS_GROUP,
    },
};

fn anonymous_pipe() -> Result<(OwnedHandle, OwnedHandle), TransportError> {
    let mut reader = ptr::null_mut();
    let mut writer = ptr::null_mut();
    // SAFETY: The output pointers are valid. No pipe handle is inheritable
    // until Command configures its selected child endpoints.
    if unsafe { CreatePipe(&mut reader, &mut writer, ptr::null(), 0) } == 0 {
        return Err(TransportError::Pipe);
    }
    // SAFETY: CreatePipe returned ownership of two valid handles on success.
    Ok(unsafe {
        (
            OwnedHandle::from_raw_handle(reader),
            OwnedHandle::from_raw_handle(writer),
        )
    })
}

fn nonblocking(handle: &OwnedHandle) -> Result<(), TransportError> {
    let mode = PIPE_NOWAIT;
    // SAFETY: This is a live parent pipe endpoint. The mode pointer is valid
    // for the call. The null pointers leave the server defaults in place.
    if unsafe { SetNamedPipeHandleState(handle.as_raw_handle(), &mode, ptr::null(), ptr::null()) }
        == 0
    {
        return Err(TransportError::Pipe);
    }
    Ok(())
}

pub(super) fn spawn(command: &mut Command) -> Result<(Child, File, File), TransportError> {
    let (child_input, parent_input) = anonymous_pipe()?;
    let (parent_output, child_output) = anonymous_pipe()?;
    nonblocking(&parent_input)?;
    nonblocking(&parent_output)?;
    let child = command
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .stdin(Stdio::from(child_input))
        .stdout(Stdio::from(child_output))
        .spawn()
        .map_err(|_| TransportError::Start)?;
    Ok((child, File::from(parent_input), File::from(parent_output)))
}

pub(super) fn read_would_block(error: &io::Error) -> bool {
    // A connected PIPE_NOWAIT reader returns ERROR_NO_DATA until bytes arrive.
    error.kind() == io::ErrorKind::WouldBlock || error.raw_os_error() == Some(ERROR_NO_DATA as i32)
}
