//! Web Proxy Auto-Discovery (WPAD) using DHCP and DNS.

use std::time::Duration;

mod dhcp;
#[cfg(target_os = "macos")]
mod dhcp_macos;
#[cfg(windows)]
mod dhcp_windows;
mod dns;

pub(crate) use dns::wpad_dns;

use crate::net_adapter::{NetAdapter, net_adapters};

// Get the WPAD url from an adapter's DHCP server
fn wpad_dhcp_adapter(adapter: &NetAdapter, timeout: Duration) -> Option<String> {
    #[cfg(windows)]
    if let Some(url) = dhcp_windows::wpad_dhcp_adapter(adapter) {
        return Some(url);
    }
    #[cfg(target_os = "macos")]
    if let Some(url) = dhcp_macos::wpad_dhcp_primary() {
        return Some(url);
    }
    dhcp::wpad_dhcp_adapter(adapter, timeout)
}

/// Discovers the proxy auto-config url by asking each connected adapter's DHCP server.
pub(crate) fn wpad_dhcp(timeout: Duration) -> Option<String> {
    net_adapters()
        .iter()
        .filter(|adapter| adapter.is_connected && adapter.ipv4().is_some())
        // Only Windows reports whether an adapter uses DHCP
        .filter(|adapter| !cfg!(windows) || adapter.is_dhcp_v4)
        .find_map(|adapter| wpad_dhcp_adapter(adapter, timeout))
}

// Clean up a WPAD url returned by a DHCP server
fn trim_wpad_url(data: &[u8]) -> Option<String> {
    // Some DHCP servers include a trailing null terminator or new line
    let url = String::from_utf8_lossy(data);
    let url = url.trim_end_matches(['\0', '\n', '\r', ' ']).trim_start();
    (!url.is_empty()).then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_wpad_url_cases() {
        assert_eq!(
            trim_wpad_url(b"http://wpad.com/wpad.dat").as_deref(),
            Some("http://wpad.com/wpad.dat")
        );
        assert_eq!(
            trim_wpad_url(b"http://wpad.com/wpad.dat\n\0").as_deref(),
            Some("http://wpad.com/wpad.dat")
        );
        assert_eq!(trim_wpad_url(b"\0"), None);
        assert_eq!(trim_wpad_url(b""), None);
    }
}
