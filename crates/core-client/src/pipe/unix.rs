use crate::TransportError;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use std::{
    fs::File,
    io,
    os::{fd::OwnedFd, unix::process::CommandExt},
    process::{Child, Command, Stdio},
};

fn nonblocking(file: &File) -> Result<(), TransportError> {
    let flags = fcntl(file, FcntlArg::F_GETFL).map_err(|_| TransportError::Pipe)?;
    fcntl(
        file,
        FcntlArg::F_SETFL(OFlag::from_bits_retain(flags) | OFlag::O_NONBLOCK),
    )
    .map_err(|_| TransportError::Pipe)?;
    Ok(())
}

pub(super) fn spawn(command: &mut Command) -> Result<(Child, File, File), TransportError> {
    let (child_input, parent_input) = io::pipe().map_err(|_| TransportError::Pipe)?;
    let (parent_output, child_output) = io::pipe().map_err(|_| TransportError::Pipe)?;
    let input = File::from(OwnedFd::from(parent_input));
    let output = File::from(OwnedFd::from(parent_output));
    nonblocking(&input)?;
    nonblocking(&output)?;
    let child = command
        .process_group(0)
        .stdin(Stdio::from(child_input))
        .stdout(Stdio::from(child_output))
        .spawn()
        .map_err(|_| TransportError::Start)?;
    Ok((child, input, output))
}

pub(super) fn read_would_block(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock
}
