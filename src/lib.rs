//! Shared display state and the versioned, thread-safe C DDX boundary.
pub mod backend;
pub mod bridge;
#[cfg(feature = "desktop")]
pub mod desktop;
pub mod layout;
#[cfg(all(feature = "desktop", target_os = "windows"))]
mod layout_windows;

#[cfg(feature = "desktop")]
pub mod selection;

pub mod auth;

pub mod window;

#[cfg(feature = "desktop")]
pub mod wm;

#[cfg(feature = "desktop")]
pub mod ime;

#[cfg(feature = "desktop")]
mod preedit;

pub mod notify;

#[cfg(feature = "desktop")]
mod geometry;

pub mod damage;

#[cfg(feature = "desktop")]
mod readiness;
#[cfg(feature = "desktop")]
pub mod x11_session;

#[cfg(feature = "desktop")]
mod renderer;
