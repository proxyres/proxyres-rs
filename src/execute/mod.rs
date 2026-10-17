//! Execute proxy auto-config (PAC) scripts.
//!
//! The script's `FindProxyForURL` function is called for a url and its return value is returned
//! as-is. The script runs with Mozilla's PAC helper functions and native implementations of
//! `dnsResolve`, `dnsResolveEx`, `myIpAddress`, `myIpAddressEx`, and `alert`.
//!
//! ```no_run
//! let script = r#"
//! function FindProxyForURL(url, host) {
//!   if (host == "simple.com")
//!     return "PROXY no-such-proxy:80";
//!   return "DIRECT";
//! }"#;
//! let list = proxyres::execute::get_proxies_for_url(script, "https://simple.com/hello-world").unwrap();
//! assert_eq!(list, "PROXY no-such-proxy:80");
//! ```

use crate::Error;
use crate::util::{split_host_port, strip_ipv6_brackets, url_host};

#[cfg(feature = "quickjs")]
mod quickjs;

/// Mozilla's JavaScript helper functions for PAC scripts.
#[cfg_attr(not(feature = "quickjs"), allow(dead_code))]
const PAC_UTILS: &str = include_str!("pac_utils.js");

/// Maximum time a PAC script may run before it is interrupted.
#[cfg_attr(not(feature = "quickjs"), allow(dead_code))]
const EXECUTE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Native helper functions exposed to PAC scripts.
#[cfg_attr(not(feature = "quickjs"), allow(dead_code))]
mod natives {
    use crate::net_util;

    /// Resolves a host to its IPv4 address, or `None` if it cannot be resolved.
    pub(super) fn dns_resolve(host: &str) -> Option<String> {
        net_util::dns_resolve(host).ok()
    }

    /// Resolves a host to its addresses, or an empty string if it cannot be resolved.
    pub(super) fn dns_resolve_ex(host: &str) -> String {
        net_util::dns_resolve_ex(host).unwrap_or_default()
    }

    /// Gets the local IPv4 address, or the loopback address if there is none.
    pub(super) fn my_ip_address() -> String {
        net_util::my_ip_address().unwrap_or_else(|| "127.0.0.1".to_string())
    }

    /// Gets the local addresses, or an empty string if there are none.
    pub(super) fn my_ip_address_ex() -> String {
        net_util::my_ip_address_ex().unwrap_or_default()
    }

    pub(super) fn alert(message: &str) {
        log::info!("PAC alert: {message}");
    }
}

/// Gets the host argument passed to `FindProxyForURL` for a url.
///
/// The host does not include the port or the brackets surrounding an IPv6 literal.
fn url_host_for_script(url: &str) -> &str {
    strip_ipv6_brackets(split_host_port(url_host(url)).0)
}

/// Whether a PAC script engine is available.
pub fn is_available() -> bool {
    cfg!(feature = "quickjs")
}

/// Executes a PAC script's `FindProxyForURL` function for a url and returns its result.
pub fn get_proxies_for_url(script: &str, url: &str) -> Result<String, Error> {
    get_proxies_for_url_with_timeout(script, url, EXECUTE_TIMEOUT)
}

fn get_proxies_for_url_with_timeout(script: &str, url: &str, timeout: std::time::Duration) -> Result<String, Error> {
    let host = url_host_for_script(url);
    #[cfg(feature = "quickjs")]
    return quickjs::find_proxy_for_url(script, url, host, timeout);
    #[cfg(not(feature = "quickjs"))]
    {
        let _ = (script, host, timeout);
        Err(Error::NoScriptEngine)
    }
}

#[cfg(all(test, feature = "quickjs"))]
mod tests {
    use super::*;
    use crate::net_util::{my_ip_address, my_ip_address_ex};

    const SCRIPT: &str = r#"
function FindProxyForURL(url, host) {
  if (host == "myip") {
    return "PROXY " + myIpAddress() + ":80";
  }
  if (host == "myipex") {
    var addresses = myIpAddressEx().split(';');
    var proxies = "";
    for (var i = 0; i < addresses.length; i++) {
      proxies += "PROXY " + addresses[i] + ":80";
      if (i != addresses.length - 1) {
        proxies += ";";
      }
    }
    return proxies;
  }
  if (host == "localhost") {
    return "PROXY " + dnsResolve(host) + ":80";
  }
  if (host == "::1") {
    return "PROXY " + dnsResolveEx(host) + ":80";
  }
  if (host == "127.0.0.1") {
    return "PROXY localhost:30";
  }
  if (isPlainHostName(host)) {
    return "HTTP plain";
  }
  if (host == "simple.com") {
    return "PROXY no-such-proxy:80";
  }
  if (shExpMatch(url, '*microsoft.com/*')) {
    return "PROXY microsoft.com:80";
  }
  if (host == "unresolvable.invalid") {
    return dnsResolve(host) === null ? "NULL" : "NOT NULL";
  }
  if (dnsDomainIs(host, ".example.com")) {
    return "PROXY example:80";
  }
  return "DIRECT";
}"#;

    fn expected_my_ip() -> String {
        format!("PROXY {}:80", my_ip_address().unwrap_or_else(|| "127.0.0.1".into()))
    }

    fn expected_my_ip_ex() -> String {
        let addresses = my_ip_address_ex().unwrap_or_default();
        addresses
            .split(';')
            .map(|address| format!("PROXY {address}:80"))
            .collect::<Vec<_>>()
            .join(";")
    }

    #[test]
    fn get_proxies_for_url_cases() {
        let tests = [
            ("your-pc", Some("HTTP plain".to_string())),
            ("127.0.0.1", Some("PROXY localhost:30".to_string())),
            ("localhost", Some("PROXY 127.0.0.1:80".to_string())),
            ("::1", Some("PROXY ::1:80".to_string())),
            ("myip", Some(expected_my_ip())),
            ("myipex", Some(expected_my_ip_ex())),
            ("http://127.0.0.1/", Some("PROXY localhost:30".to_string())),
            ("http://simple.com/", Some("PROXY no-such-proxy:80".to_string())),
            ("http://example2.com/", Some("DIRECT".to_string())),
            ("http://microsoft.com/test", Some("PROXY microsoft.com:80".to_string())),
            ("file:///c:/test", None),
            ("file:////home/test", None),
            // Host does not include the port
            ("http://www.example.com:8080/", Some("PROXY example:80".to_string())),
            ("http://[::1]:8080/", Some("PROXY ::1:80".to_string())),
            // Unresolvable hosts resolve to null
            ("http://unresolvable.invalid/", Some("NULL".to_string())),
        ];
        for (url, expected) in tests {
            let list = get_proxies_for_url(SCRIPT, url);
            assert!(list.is_ok(), "url: {url} error: {list:?}");
            if let Some(expected) = expected {
                assert_eq!(list.unwrap(), expected, "url: {url}");
            }
        }
    }

    #[test]
    fn url_with_quotes_is_not_evaluated() {
        let script = "function FindProxyForURL(url, host) { return url; }";
        let url = r#"http://a/");throw "injected";//"#;
        assert_eq!(get_proxies_for_url(script, url).as_deref(), Ok(url));
    }

    #[test]
    fn syntax_error() {
        let error = get_proxies_for_url("function FindProxyForURL(url, host) {", "http://a/").unwrap_err();
        assert!(matches!(error, Error::Script(_)), "{error:?}");
    }

    #[test]
    fn missing_function() {
        let error = get_proxies_for_url("var a = 1;", "http://a/").unwrap_err();
        assert!(matches!(error, Error::Script(_)), "{error:?}");
    }

    #[test]
    fn exception() {
        let script = "function FindProxyForURL(url, host) { throw new Error('boom'); }";
        let error = get_proxies_for_url(script, "http://a/").unwrap_err();
        assert!(
            matches!(&error, Error::Script(message) if message.contains("boom")),
            "{error:?}"
        );
    }

    #[test]
    fn non_string_result() {
        let script = "function FindProxyForURL(url, host) { return undefined; }";
        assert!(matches!(
            get_proxies_for_url(script, "http://a/"),
            Err(Error::Script(_))
        ));
    }

    #[test]
    fn infinite_loop_is_interrupted() {
        let script = "function FindProxyForURL(url, host) { while (true) {} }";
        let timeout = std::time::Duration::from_millis(500);
        let start = std::time::Instant::now();
        let result = get_proxies_for_url_with_timeout(script, "http://a/", timeout);
        assert!(matches!(result, Err(Error::Script(_))), "{result:?}");
        assert!(start.elapsed() < timeout * 4);
    }
}
