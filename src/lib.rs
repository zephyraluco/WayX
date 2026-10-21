//! Shared display state and the versioned, thread-safe C DDX boundary.
pub mod backend;
pub mod bridge;
#[cfg(feature = "desktop")]
pub mod desktop;
#[cfg(feature = "ssh")]
pub mod ssh;

#[cfg(feature = "desktop")]
pub mod selection;

pub mod auth;
