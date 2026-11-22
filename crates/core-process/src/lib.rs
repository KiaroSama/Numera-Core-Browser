//! Standalone profile admission, durable claims and native process ownership primitives.
mod admission;
pub use admission::{Claim, Lifecycle, StandaloneLease};

#[cfg(any(target_os = "linux", windows))]
mod session;
#[cfg(any(target_os = "linux", windows))]
pub use session::{BrowserProcess, SessionPaths, profile_package, snapshot_profile};

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(windows)]
pub mod windows;
