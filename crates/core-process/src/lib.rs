//! Standalone profile admission, durable claims and native process ownership primitives.
mod admission;
pub use admission::{Claim, Lifecycle, StandaloneLease};

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(windows)]
pub mod windows;
