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
    Foundation::{ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_NOT_CONNECTED},
    Storage::FileSystem::{ReadFile, WriteFile},
    System::{
        Pipes::{CreatePipe, PIPE_NOWAIT, SetNamedPipeHandleState},
        Threading::CREATE_NEW_PROCESS_GROUP,
    },
};

const PIPE_BUFFER_BYTES: u32 = 16 * 1024;
const WRITE_CHUNK_BYTES: usize = 4 * 1024;

fn anonymous_pipe() -> Result<(OwnedHandle, OwnedHandle), TransportError> {
    let mut reader = ptr::null_mut();
    let mut writer = ptr::null_mut();
    // SAFETY: The output pointers are valid. No pipe handle is inheritable
    // until Command configures its selected child endpoints.
    if unsafe { CreatePipe(&mut reader, &mut writer, ptr::null(), PIPE_BUFFER_BYTES) } == 0 {
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

pub(super) fn read(pipe: &mut File, buffer: &mut [u8]) -> io::Result<usize> {
    let length = buffer.len().min(u32::MAX as usize) as u32;
    let mut count = 0;
    // SAFETY: The parent handle is synchronous and set to PIPE_NOWAIT. The
    // buffer and byte count remain writable for the duration of the call.
    let read = unsafe {
        ReadFile(
            pipe.as_raw_handle(),
            buffer.as_mut_ptr(),
            length,
            &mut count,
            ptr::null_mut(),
        )
    };
    if read != 0 {
        // Win32 can return success with no bytes after a zero-length pipe write.
        return if count == 0 {
            Err(io::ErrorKind::WouldBlock.into())
        } else {
            Ok(count as usize)
        };
    }
    // File::read treats ERROR_NO_DATA as EOF because Rust maps it to BrokenPipe.
    // On a connected PIPE_NOWAIT reader it means there is nothing to read yet.
    let error = io::Error::last_os_error();
    match error.raw_os_error().map(|code| code as u32) {
        Some(ERROR_NO_DATA) => Err(io::ErrorKind::WouldBlock.into()),
        Some(ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED) => Ok(0),
        _ => Err(error),
    }
}

pub(super) fn write(pipe: &mut File, buffer: &[u8]) -> io::Result<usize> {
    // A full-frame request can exceed the pipe quota and report no progress.
    // Small writes let the caller advance the frame and check its deadline.
    let length = buffer.len().min(WRITE_CHUNK_BYTES) as u32;
    let mut count = 0;
    // SAFETY: The parent handle is synchronous and set to PIPE_NOWAIT. The
    // buffer and byte count are valid for the call, which accepts a partial write.
    let written = unsafe {
        WriteFile(
            pipe.as_raw_handle(),
            buffer.as_ptr(),
            length,
            &mut count,
            ptr::null_mut(),
        )
    };
    if written == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(count as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_connected_pipe_is_pending_until_data_or_writer_closure() {
        let (reader, writer) = anonymous_pipe().unwrap();
        nonblocking(&reader).unwrap();
        nonblocking(&writer).unwrap();
        let mut reader = File::from(reader);
        let mut writer = File::from(writer);
        let mut bytes = [0; 8];

        assert_eq!(
            read(&mut reader, &mut bytes).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(write(&mut writer, b"data").unwrap(), 4);
        assert_eq!(read(&mut reader, &mut bytes).unwrap(), 4);
        assert_eq!(&bytes[..4], b"data");
        assert_eq!(
            read(&mut reader, &mut bytes).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        drop(writer);
        assert_eq!(read(&mut reader, &mut bytes).unwrap(), 0);
    }

    #[test]
    fn a_large_write_queues_bytes_before_backpressure_and_resumes_after_reading() {
        let (reader, writer) = anonymous_pipe().unwrap();
        nonblocking(&reader).unwrap();
        nonblocking(&writer).unwrap();
        let mut reader = File::from(reader);
        let mut writer = File::from(writer);
        let payload = vec![b'x'; core_contracts::MAX_FRAME];

        let mut queued = 0;
        while queued < payload.len() {
            let count = write(&mut writer, &payload[queued..]).unwrap();
            if count == 0 {
                break;
            }
            queued += count;
        }
        assert!(
            queued > 0 && queued < payload.len(),
            "queued {queued} bytes before backpressure"
        );

        let mut received = vec![0; queued];
        let mut consumed = 0;
        while consumed < received.len() {
            let count = read(&mut reader, &mut received[consumed..]).unwrap();
            assert!(count > 0, "pipe reached EOF with the writer still open");
            consumed += count;
        }
        assert_eq!(received, payload[..queued]);
        assert_eq!(write(&mut writer, b"next").unwrap(), 4);
        let mut next = [0; 4];
        assert_eq!(read(&mut reader, &mut next).unwrap(), 4);
        assert_eq!(&next, b"next");
    }
}
