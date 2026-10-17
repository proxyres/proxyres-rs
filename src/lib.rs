//! Cross-platform proxy resolution.
//!
//! This crate is a Rust port of [proxyres](https://github.com/nmoinvaz/proxyres). It reads the user's
//! proxy configuration from the operating system, evaluates proxy auto-config (PAC) scripts, and
//! resolves the list of proxies to use for a given URL.

mod net_util;
mod util;
