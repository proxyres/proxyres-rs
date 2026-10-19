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

use std::time::Duration;

use crate::Error;
use crate::util::{split_host_port, strip_ipv6_brackets, url_host};

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod javascriptcore;
#[cfg(target_os = "linux")]
mod javascriptcore_gtk;
#[cfg(feature = "quickjs")]
mod quickjs;
#[cfg(windows)]
mod wsh;

/// Mozilla's JavaScript helper functions for PAC scripts.
#[cfg_attr(
    not(any(
        feature = "quickjs",
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        windows
    )),
    allow(dead_code)
)]
const PAC_UTILS: &str = include_str!("pac_utils.js");

/// Maximum time a PAC script may run in engines that can interrupt it.
#[cfg_attr(not(feature = "quickjs"), allow(dead_code))]
const EXECUTE_TIMEOUT: Duration = Duration::from_secs(30);

/// Native helper functions exposed to PAC scripts.
#[cfg_attr(
    not(any(
        feature = "quickjs",
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        windows
    )),
    allow(dead_code)
)]
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

/// PAC script engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Engine {
    /// The bundled QuickJS engine, available with the `quickjs` feature.
    QuickJs,
    /// JavaScriptCore loaded at run-time, available on macOS and on Linux with JavaScriptCoreGTK
    /// installed.
    JavaScriptCore,
    /// Windows Script Host using the JScript engine, available on Windows.
    WindowsScriptHost,
}

impl Engine {
    /// Gets the engines available on this system in order of preference.
    pub fn available() -> Vec<Engine> {
        [Engine::QuickJs, Engine::JavaScriptCore, Engine::WindowsScriptHost]
            .into_iter()
            .filter(|engine| engine.is_available())
            .collect()
    }

    /// Whether the engine is available on this system.
    pub fn is_available(self) -> bool {
        match self {
            Engine::QuickJs => cfg!(feature = "quickjs"),
            #[cfg(target_os = "linux")]
            Engine::JavaScriptCore => javascriptcore_gtk::is_available(),
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            Engine::JavaScriptCore => javascriptcore::is_available(),
            #[cfg(windows)]
            Engine::WindowsScriptHost => wsh::is_available(),
            _ => false,
        }
    }
}

/// Whether a PAC script engine is available.
pub fn is_available() -> bool {
    !Engine::available().is_empty()
}

/// Executes a PAC script's `FindProxyForURL` function for a url and returns its result.
///
/// The first available engine is used, preferring the bundled engine over the system engines.
pub fn get_proxies_for_url(script: &str, url: &str) -> Result<String, Error> {
    let engine = Engine::available().into_iter().next().ok_or(Error::NoScriptEngine)?;
    get_proxies_for_url_with(engine, script, url)
}

/// Executes a PAC script's `FindProxyForURL` function for a url using a specific engine.
pub fn get_proxies_for_url_with(engine: Engine, script: &str, url: &str) -> Result<String, Error> {
    execute(engine, script, url, EXECUTE_TIMEOUT)
}

fn execute(engine: Engine, script: &str, url: &str, timeout: Duration) -> Result<String, Error> {
    let host = url_host_for_script(url);
    match engine {
        #[cfg(feature = "quickjs")]
        Engine::QuickJs => quickjs::find_proxy_for_url(script, url, host, timeout),
        #[cfg(target_os = "linux")]
        Engine::JavaScriptCore => javascriptcore_gtk::find_proxy_for_url(script, url, host),
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        Engine::JavaScriptCore => javascriptcore::find_proxy_for_url(script, url, host),
        #[cfg(windows)]
        Engine::WindowsScriptHost => wsh::find_proxy_for_url(script, url, host),
        _ => {
            let _ = (script, host, timeout);
            Err(Error::NoScriptEngine)
        }
    }
}

#[cfg(test)]
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

    // Engines to test, which always include the bundled engine when it is enabled
    fn engines() -> Vec<Engine> {
        let engines = Engine::available();
        println!("engines: {engines:?}");
        assert!(!cfg!(feature = "quickjs") || engines.contains(&Engine::QuickJs));
        engines
    }

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
        for engine in engines() {
            for (url, expected) in &tests {
                let list = get_proxies_for_url_with(engine, SCRIPT, url);
                assert!(list.is_ok(), "engine: {engine:?} url: {url} error: {list:?}");
                if let Some(expected) = expected {
                    assert_eq!(&list.unwrap(), expected, "engine: {engine:?} url: {url}");
                }
            }
        }
    }

    #[test]
    fn url_with_quotes_is_not_evaluated() {
        let script = "function FindProxyForURL(url, host) { return url; }";
        let url = r#"http://a/");throw "injected";//"#;
        for engine in engines() {
            assert_eq!(
                get_proxies_for_url_with(engine, script, url).as_deref(),
                Ok(url),
                "engine: {engine:?}"
            );
        }
    }

    #[test]
    fn syntax_error() {
        for engine in engines() {
            let result = get_proxies_for_url_with(engine, "function FindProxyForURL(url, host) {", "http://a/");
            assert!(matches!(result, Err(Error::Script(_))), "engine: {engine:?} {result:?}");
        }
    }

    #[test]
    fn missing_function() {
        for engine in engines() {
            let result = get_proxies_for_url_with(engine, "var a = 1;", "http://a/");
            assert!(matches!(result, Err(Error::Script(_))), "engine: {engine:?} {result:?}");
        }
    }

    #[test]
    fn exception() {
        let script = "function FindProxyForURL(url, host) { throw new Error('boom'); }";
        for engine in engines() {
            let result = get_proxies_for_url_with(engine, script, "http://a/");
            assert!(
                matches!(&result, Err(Error::Script(message)) if message.contains("boom")),
                "engine: {engine:?} {result:?}"
            );
        }
    }

    #[test]
    fn thrown_value() {
        let script = "function FindProxyForURL(url, host) { throw 42; }";
        for engine in engines() {
            let result = get_proxies_for_url_with(engine, script, "http://a/");
            assert!(matches!(result, Err(Error::Script(_))), "engine: {engine:?} {result:?}");
        }
    }

    #[test]
    fn undefined_function() {
        let script = "function FindProxyForURL(url, host) { return notDefined(host); }";
        for engine in engines() {
            let result = get_proxies_for_url_with(engine, script, "http://a/");
            assert!(matches!(result, Err(Error::Script(_))), "engine: {engine:?} {result:?}");
        }
    }

    #[test]
    fn alert() {
        // Alert accepts any value or none and only logs it
        let script = r#"function FindProxyForURL(url, host) { alert("hello"); alert(42); alert(); return "DIRECT"; }"#;
        for engine in engines() {
            assert_eq!(
                get_proxies_for_url_with(engine, script, "http://a/").as_deref(),
                Ok("DIRECT"),
                "engine: {engine:?}"
            );
        }
    }

    #[test]
    fn concurrent_executions() {
        // Executions on different threads keep their own native functions and results
        for engine in engines() {
            std::thread::scope(|scope| {
                for i in 0..8 {
                    scope.spawn(move || {
                        let script = format!(
                            r#"function FindProxyForURL(url, host) {{ return "PROXY t{i}-" + dnsResolve("127.0.0.1") + ":80"; }}"#
                        );
                        let expected = format!("PROXY t{i}-127.0.0.1:80");
                        for _ in 0..10 {
                            assert_eq!(
                                get_proxies_for_url_with(engine, &script, "http://a/").as_deref(),
                                Ok(expected.as_str()),
                                "engine: {engine:?}"
                            );
                        }
                    });
                }
            });
        }
    }

    #[test]
    fn non_string_result() {
        let script = "function FindProxyForURL(url, host) { return undefined; }";
        for engine in engines() {
            let result = get_proxies_for_url_with(engine, script, "http://a/");
            assert!(matches!(result, Err(Error::Script(_))), "engine: {engine:?} {result:?}");
        }
    }

    #[test]
    fn unavailable_engine() {
        let unavailable = [Engine::QuickJs, Engine::JavaScriptCore, Engine::WindowsScriptHost]
            .into_iter()
            .filter(|engine| !engine.is_available());
        for engine in unavailable {
            assert_eq!(
                get_proxies_for_url_with(engine, SCRIPT, "http://a/"),
                Err(Error::NoScriptEngine)
            );
        }
    }

    #[cfg(feature = "quickjs")]
    #[test]
    fn infinite_loop_is_interrupted() {
        let script = "function FindProxyForURL(url, host) { while (true) {} }";
        let timeout = Duration::from_millis(500);
        let start = std::time::Instant::now();
        let result = execute(Engine::QuickJs, script, "http://a/", timeout);
        assert!(matches!(result, Err(Error::Script(_))), "{result:?}");
        assert!(start.elapsed() < timeout * 4);
    }
}
