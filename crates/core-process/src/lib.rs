//! Standalone profile admission with an OS lock and durable claim.
//! Browser spawning, descendant supervision, recovery repair and manager grants
//! are separate runtime gates and are not implemented by this module.
mod admission;
pub use admission::{Claim, Lifecycle, StandaloneLease};

#[cfg(windows)]
pub mod windows;
