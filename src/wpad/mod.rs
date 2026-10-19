//! Web Proxy Auto-Discovery (WPAD) using DHCP and DNS.

use std::time::Duration;

mod dhcp;
#[cfg(target_os = "macos")]
mod dhcp_macos;
#[cfg(windows)]
mod dhcp_windows;
mod dns;
#[cfg(unix)]
mod dns_unix;
#[cfg(windows)]
mod dns_windows;

pub(crate) use dns::wpad_dns;

use crate::net_adapter::{NetAdapter, net_adapters};
use crate::util::url_scheme;

// Get the WPAD url from an adapter's DHCP server
fn wpad_dhcp_adapter(adapter: &NetAdapter, timeout: Duration) -> Option<String> {
    #[cfg(windows)]
    if let Some(url) = dhcp_windows::wpad_dhcp_adapter(adapter) {
        return Some(url);
    }
    dhcp::wpad_dhcp_adapter(adapter, timeout)
}

/// Discovers the proxy auto-config url by asking each connected adapter's DHCP server.
pub(crate) fn wpad_dhcp(timeout: Duration) -> Option<String> {
    // The lease of the primary service doesn't depend on the adapter
    #[cfg(target_os = "macos")]
    if let Some(url) = dhcp_macos::wpad_dhcp_primary().and_then(allowed_url) {
        return Some(url);
    }

    let mut adapters: Vec<NetAdapter> = net_adapters()
        .into_iter()
        .filter(|adapter| adapter.is_connected && adapter.ipv4().is_some())
        // Only Windows reports whether an adapter uses DHCP
        .filter(|adapter| !cfg!(windows) || adapter.is_dhcp_v4)
        .collect();
    // Query the primary adapter first since it carries proxied traffic
    adapters.sort_by_key(|adapter| !adapter.is_primary);
    adapters
        .iter()
        .find_map(|adapter| wpad_dhcp_adapter(adapter, timeout).and_then(allowed_url))
}

// WPAD fetches the config file over HTTP, and any host on the network can answer DHCP. Chromium's
// PacFileFetcherImpl::IsUrlSchemeAllowed likewise only fetches PAC urls over http, https and data.
fn allowed_url(url: String) -> Option<String> {
    let scheme = url_scheme(&url).unwrap_or_default();
    if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
        return Some(url);
    }
    log::warn!("Ignoring WPAD url with unsupported scheme {url}");
    None
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

    #[test]
    fn allowed_url_cases() {
        let tests = [
            ("http://wpad.example.com/wpad.dat", true),
            ("HTTPS://wpad.example.com/wpad.dat", true),
            ("file:///etc/proxy.pac", false),
            ("ftp://wpad.example.com/wpad.dat", false),
            // Url without a scheme
            ("wpad.example.com/wpad.dat", false),
        ];
        for (url, allowed) in tests {
            assert_eq!(allowed_url(url.to_string()).is_some(), allowed, "url: {url}");
        }
    }

    #[test]
    fn dhcp_discovery_returns() {
        // Calls the operating system's DHCP interfaces, which find no WPAD url on most networks
        println!("wpad: {:?}", wpad_dhcp(Duration::from_millis(500)));
    }
}
