//! Cross-platform proxy resolution.
//!
//! This crate is a Rust port of [proxyres](https://github.com/proxyres/proxyres). It reads the user's
//! proxy configuration from the operating system, evaluates proxy auto-config (PAC) scripts, and
//! resolves the list of proxies to use for a given URL.

pub mod config;
mod error;
pub mod execute;
pub mod resolver;

pub use error::Error;

mod fetch;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod macos;
mod net_adapter;
mod net_util;
mod threadpool;
mod util;
#[cfg(any(target_os = "linux", test))]
mod util_linux;
#[cfg(any(windows, test))]
mod util_windows;
mod wpad;
