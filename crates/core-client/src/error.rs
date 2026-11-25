use core_contracts::ErrorCode;
use std::fmt;

/// An NCB-1 rejection or a local failure that invalidates the connection.
#[derive(Debug, PartialEq, Eq)]
pub enum ClientError {
    Request(ErrorCode),
    Transport(TransportError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    Start,
    Pipe,
    Deadline,
    FrameLimit,
    Closed,
    InvalidResponse,
    MismatchedResponse,
    InvalidCapabilityScope,
    EventGap,
    CleanupTimeout,
    ExitStatusUnavailable,
}

impl From<TransportError> for ClientError {
    fn from(error: TransportError) -> Self {
        Self::Transport(error)
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Request(code) => return write!(formatter, "NCB-1 request failed ({code:?})"),
            Self::Transport(TransportError::Start) => "Could not start Numera Core",
            Self::Transport(TransportError::Pipe) => "NCB-1 pipe I/O failed",
            Self::Transport(TransportError::Deadline) => "NCB-1 request deadline expired",
            Self::Transport(TransportError::FrameLimit) => "NCB-1 response exceeds the frame limit",
            Self::Transport(TransportError::Closed) => "NCB-1 connection is closed",
            Self::Transport(TransportError::InvalidResponse) => "Invalid NCB-1 response",
            Self::Transport(TransportError::MismatchedResponse) => {
                "NCB-1 response does not correspond to the request"
            }
            Self::Transport(TransportError::InvalidCapabilityScope) => {
                "Invalid NCB-1 capability scope"
            }
            Self::Transport(TransportError::EventGap) => "NCB-1 session event sequence has a gap",
            Self::Transport(TransportError::CleanupTimeout) => {
                "Numera Core did not exit within the cleanup budget"
            }
            Self::Transport(TransportError::ExitStatusUnavailable) => {
                "Could not observe Numera Core's exit status"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ClientError {}
