//! Cross-platform proxy resolution.
//!
//! This crate is a Rust port of [proxyres](https://github.com/nmoinvaz/proxyres). It reads the user's
//! proxy configuration from the operating system, evaluates proxy auto-config (PAC) scripts, and
//! resolves the list of proxies to use for a given URL.

pub mod config;
mod error;
pub mod execute;

pub use error::Error;

#[cfg(target_os = "macos")]
mod cf;
mod net_adapter;
mod net_util;
mod util;
#[cfg(any(target_os = "linux", test))]
mod util_linux;
#[cfg(windows)]
mod util_win;
