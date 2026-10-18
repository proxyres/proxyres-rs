//! String helpers for urls, proxy lists, and proxy bypass rules.

use crate::net_util::{is_ipv4_in_cidr_range, is_ipv6_in_cidr_range};

/// Maximum size of a PAC script that will be downloaded.
pub(crate) const SCRIPT_MAX: usize = 256 * 1024;

fn starts_with_ignore_case(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

// Remainder of the url after the scheme separator
fn after_scheme(url: &str) -> &str {
    url.find("://").map_or(url, |i| &url[i + 3..])
}

/// Returns the scheme of a url, if it has one.
pub(crate) fn url_scheme(url: &str) -> Option<&str> {
    url.find("://").map(|end| &url[..end])
}

/// Returns the host of a url including its port, without any user info.
pub(crate) fn url_host(url: &str) -> &str {
    let rest = after_scheme(url);
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    authority.rfind('@').map_or(authority, |i| &authority[i + 1..])
}

/// Returns the path of a url, or a slash if it has none.
pub(crate) fn url_path(url: &str) -> &str {
    let rest = after_scheme(url);
    rest.find('/').map_or("/", |i| &rest[i..])
}

// Parse a decimal port number, ignoring anything after the digits
fn parse_port(s: &str) -> Option<u16> {
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    s[..digits].parse::<u16>().ok().filter(|&port| port != 0)
}

/// Splits a host into its name and port, handling bracketed IPv6 literals.
pub(crate) fn split_host_port(host: &str) -> (&str, Option<u16>) {
    if host.starts_with('[') {
        return match host.find(']') {
            Some(end) => (&host[..=end], host[end + 1..].strip_prefix(':').and_then(parse_port)),
            None => (host, None),
        };
    }
    // Bare IPv6 literals have more than one colon and never carry a port
    match host.split_once(':') {
        Some((name, port)) if !port.contains(':') => (name, parse_port(port)),
        _ => (host, None),
    }
}

/// Returns the port of a host, if it has one.
pub(crate) fn host_port(host: &str) -> Option<u16> {
    split_host_port(host).1
}

/// Removes the brackets surrounding an IPv6 literal.
pub(crate) fn strip_ipv6_brackets(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host)
}

/// Returns the scheme commonly associated with a port.
pub(crate) fn port_scheme(port: u16) -> Option<&'static str> {
    match port {
        80 => Some("http"),
        443 => Some("https"),
        1080 => Some("socks"),
        21 => Some("ftp"),
        _ => None,
    }
}

/// Returns the default port for a scheme.
pub(crate) fn scheme_default_port(scheme: &str) -> Option<u16> {
    match scheme.to_ascii_lowercase().as_str() {
        "http" => Some(80),
        "https" => Some(443),
        "socks" => Some(1080),
        "ftp" => Some(21),
        _ => None,
    }
}

/// Creates a url from a host, appending the default port if the host does not have one.
///
/// Only the scheme of a url passed as `scheme` is used. A bare scheme falls back to `http`, which
/// matches the C library.
pub(crate) fn url_from_host(scheme: &str, host: &str) -> String {
    let real_scheme = url_scheme(scheme).unwrap_or("http");
    let mut url = if host.contains("://") {
        host.to_string()
    } else {
        format!("{real_scheme}://{host}")
    };
    url.truncate(url.trim_end_matches('/').len());

    // Append the default port of the host's scheme if no port is specified
    let authority = url_host(&url);
    if split_host_port(authority).0.len() == authority.len() {
        if let Some(port) = scheme_default_port(url_scheme(host).unwrap_or("http")) {
            url.push_str(&format!(":{port}"));
        }
    }
    url
}

/// Converts the proxy list returned by `FindProxyForURL` into a comma-separated list of uris.
///
/// The proxy list contains one or more proxies separated by semicolons:
///
/// ```text
/// returnValue = type host,":",port,[{ ";",returnValue }];
/// type        = "DIRECT" | "PROXY" | "SOCKS" | "HTTP" | "HTTPS" | "SOCKS4" | "SOCKS5"
/// ```
///
/// When `default_scheme` is not set, the scheme for `PROXY` is chosen based on the port.
pub(crate) fn convert_proxy_list_to_uri_list(proxy_list: &str, default_scheme: Option<&str>) -> String {
    const TYPES: [(&str, Option<&str>); 7] = [
        ("PROXY ", None),
        ("DIRECT", Some("direct")),
        ("HTTP ", Some("http")),
        ("HTTPS ", Some("https")),
        ("SOCKS ", Some("socks")),
        ("SOCKS4 ", Some("socks4")),
        ("SOCKS5 ", Some("socks5")),
    ];

    let mut uris = Vec::new();
    for config in proxy_list.split(';').map(str::trim).filter(|config| !config.is_empty()) {
        let mut scheme = default_scheme;
        let mut host = config;
        for (prefix, type_scheme) in TYPES {
            if starts_with_ignore_case(config, prefix) {
                host = config[prefix.len()..].trim();
                scheme = type_scheme.or(scheme);
                break;
            }
        }
        if scheme == Some("direct") {
            host = "";
        }
        // Use scheme based on port for type PROXY
        let scheme = scheme.unwrap_or_else(|| port_scheme(host_port(host).unwrap_or(80)).unwrap_or("http"));
        uris.push(format!("{scheme}://{host}"));
    }
    uris.join(",")
}

/// Compares a string against a pattern where `*` matches any sequence of characters.
pub(crate) fn wildcard_match(text: &str, pattern: &str, ignore_case: bool) -> bool {
    let (text, pattern) = (text.as_bytes(), pattern.as_bytes());
    let eq = |a: u8, b: u8| {
        if ignore_case {
            a.eq_ignore_ascii_case(&b)
        } else {
            a == b
        }
    };
    let (mut t, mut p) = (0, 0);
    // Position of the last star in the pattern and the text position it is matched up to
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == b'*' {
            star = Some((p, t));
            p += 1;
        } else if p < pattern.len() && eq(text[t], pattern[p]) {
            t += 1;
            p += 1;
        } else if let Some((star_p, star_t)) = star {
            // Let the last star consume one more character and try again
            star = Some((star_p, star_t + 1));
            p = star_p + 1;
            t = star_t + 1;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == b'*')
}

/// Evaluates whether or not the proxy should be bypassed for a url.
///
/// Rules follow Chromium's proxy bypass rules
/// <https://chromium.googlesource.com/chromium/src/+/HEAD/net/docs/proxy.md#Proxy-bypass-rules>:
/// localhost is bypassed unless `<-loopback>` is present, `<local>` bypasses simple hostnames,
/// rules may use `*` wildcards, a leading `.` matches subdomains, a port restricts the match, and
/// cidr notation matches ip ranges.
pub(crate) fn should_bypass_proxy(url: &str, bypass_list: Option<&str>) -> bool {
    let (host, port) = split_host_port(url_host(url));
    let is_local = host == "127.0.0.1" || host == "[::1]" || host.eq_ignore_ascii_case("localhost");
    let Some(bypass_list) = bypass_list else {
        return is_local;
    };

    let is_simple = !host.contains('.');
    let rules: Vec<&str> = bypass_list
        .split(',')
        .map(str::trim)
        .filter(|rule| !rule.is_empty())
        .collect();

    // By default don't allow localhost urls to go through proxy
    if is_local && !rules.iter().any(|rule| rule.eq_ignore_ascii_case("<-loopback>")) {
        return true;
    }

    rules.iter().any(|rule| {
        if rule.eq_ignore_ascii_case("<local>") {
            return is_simple;
        }
        if rule.contains('/') {
            let ip = strip_ipv6_brackets(host);
            return is_ipv4_in_cidr_range(ip, rule) || is_ipv6_in_cidr_range(ip, rule);
        }
        let (rule_host, rule_port) = split_host_port(rule);
        // Rules starting with a dot match subdomains
        let pattern = if rule_host.starts_with('.') {
            format!("*{rule_host}")
        } else {
            rule_host.to_string()
        };
        if !wildcard_match(host, &pattern, true) {
            return false;
        }
        match rule_port {
            Some(rule_port) => {
                // Infer the port from the url scheme if the url does not specify one
                let port = port.or_else(|| scheme_default_port(url_scheme(url).unwrap_or("http")));
                port == Some(rule_port)
            }
            None => true,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_host_cases() {
        let tests = [
            ("https://google.com/", "google.com"),
            ("https://google.com", "google.com"),
            ("google.com", "google.com"),
            ("google.com/", "google.com"),
            ("https://u:p@google.com/", "google.com"),
            ("https://u:p@www.google.com", "www.google.com"),
            // User info is only parsed from the authority
            ("http://example.com/login@evil.com", "example.com"),
            ("http://example.com?redirect=a@b.com", "example.com"),
            ("http://example.com:8080/", "example.com:8080"),
        ];
        for (url, expected) in tests {
            assert_eq!(url_host(url), expected, "url: {url}");
        }
    }

    #[test]
    fn url_path_cases() {
        assert_eq!(url_path("http://example.com"), "/");
        assert_eq!(url_path("http://example.com/wpad.dat"), "/wpad.dat");
        assert_eq!(url_path("example.com/a/b"), "/a/b");
    }

    #[test]
    fn split_host_port_cases() {
        let tests = [
            ("example.com", ("example.com", None)),
            ("example.com:8080", ("example.com", Some(8080))),
            ("example.com:010", ("example.com", Some(10))),
            ("example.com:08080", ("example.com", Some(8080))),
            ("example.com:0", ("example.com", None)),
            ("[::1]", ("[::1]", None)),
            ("[::1]:80", ("[::1]", Some(80))),
            ("::1", ("::1", None)),
            ("::ffff:c0a8:1", ("::ffff:c0a8:1", None)),
        ];
        for (host, expected) in tests {
            assert_eq!(split_host_port(host), expected, "host: {host}");
        }
    }

    #[test]
    fn url_from_host_cases() {
        let tests = [
            // Http no scheme no port
            ("http", "1.1.1.1", "http://1.1.1.1:80"),
            ("http", "yahoo.com", "http://yahoo.com:80"),
            // Http no scheme with port
            ("http", "1.1.1.1:80", "http://1.1.1.1:80"),
            // Https no scheme no port
            ("https", "127.0.0.1", "http://127.0.0.1:80"),
            ("https://google.com", "google.com", "https://google.com:80"),
            // Http scheme no port
            ("http", "http://bing.com", "http://bing.com:80"),
            // Https scheme no port
            ("https", "https://bing.com", "https://bing.com:443"),
            // Https scheme with port
            ("https", "https://bing.com:443", "https://bing.com:443"),
            // Http mixed-scheme
            ("http", "https://bing.com", "https://bing.com:443"),
            // Https mixed-scheme
            ("https", "http://bing.com", "http://bing.com:80"),
            // Trailing slash is removed
            ("http", "http://127.0.0.1:8000/", "http://127.0.0.1:8000"),
            // IPv6 literal without port
            ("http", "[::1]", "http://[::1]:80"),
        ];
        for (scheme, host, expected) in tests {
            assert_eq!(url_from_host(scheme, host), expected, "scheme: {scheme} host: {host}");
        }
    }

    #[test]
    fn convert_proxy_list_to_uri_list_cases() {
        let tests = [
            ("DIRECT", None, "direct://"),
            ("PROXY 127.0.0.1:80", None, "http://127.0.0.1:80"),
            ("PROXY 127.0.0.1:8080", None, "http://127.0.0.1:8080"),
            ("PROXY 127.0.0.1:443", None, "https://127.0.0.1:443"),
            ("PROXY myproxy0.com:90", Some("http"), "http://myproxy0.com:90"),
            ("PROXY myproxy1.com:90", Some("https"), "https://myproxy1.com:90"),
            (
                "HTTP myproxy1.com:80;SOCKS myproxy2.com:1080",
                None,
                "http://myproxy1.com:80,socks://myproxy2.com:1080",
            ),
            (
                "HTTP myproxy3.com:80;SOCKS myproxy4.com:1080;DIRECT",
                None,
                "http://myproxy3.com:80,socks://myproxy4.com:1080,direct://",
            ),
            (
                "PROXY proxy1.example.com:80; PROXY proxy2.example.com:8080",
                None,
                "http://proxy1.example.com:80,http://proxy2.example.com:8080",
            ),
            // Empty entries and surrounding whitespace are ignored
            ("PROXY a:8080;", Some("http"), "http://a:8080"),
            ("PROXY a:8080; DIRECT ", Some("http"), "http://a:8080,direct://"),
            ("", None, ""),
        ];
        for (proxy_list, default_scheme, expected) in tests {
            assert_eq!(
                convert_proxy_list_to_uri_list(proxy_list, default_scheme),
                expected,
                "proxy list: {proxy_list}"
            );
        }
    }

    #[test]
    fn convert_proxy_list_to_uri_list_long() {
        let proxy_list = "DIRECT;".repeat(1000);
        let uri_list = convert_proxy_list_to_uri_list(&proxy_list, None);
        assert_eq!(uri_list.split(',').count(), 1000);
    }

    #[test]
    fn wildcard_match_cases() {
        assert!(wildcard_match("google.com", "*google.com", true));
        assert!(wildcard_match("my.google.com", "*.google.com", true));
        assert!(!wildcard_match("google.com", "*.google.com", true));
        assert!(wildcard_match("GOOGLE.com", "google.*", true));
        assert!(!wildcard_match("GOOGLE.com", "google.*", false));
        assert!(wildcard_match("abc", "a*c", false));
        assert!(wildcard_match("abc", "abc*", false));
        assert!(!wildcard_match("abc", "abc*x", false));
        assert!(wildcard_match("", "*", false));
        assert!(!wildcard_match("", "a", false));
    }

    #[test]
    fn should_bypass_proxy_cases() {
        let tests = [
            // Bypass due to localhost hostname
            ("http://localhost/", Some(""), true),
            ("http://localhost/", None, true),
            // Bypass due to localhost ipv4 address
            ("http://127.0.0.1/", Some(""), true),
            ("http://127.0.0.1/", None, true),
            // Bypass due to localhost ipv6 address
            ("http://[::1]/", Some(""), true),
            ("http://[::1]/", None, true),
            // Bypass due to <local> on simple hostnames
            ("http://apple/", Some("<local>"), true),
            // Don't bypass due to <local> only applying to simple hostnames
            ("http://apple.com/", Some("<local>"), false),
            // Don't bypass due to <-loopback> on localhost ip
            ("http://localhost/", Some("<-loopback>"), false),
            ("http://127.0.0.1/", Some("<-loopback>"), false),
            ("http://[::1]/", Some("<-loopback>"), false),
            // Don't bypass due to <-loopback> not applying to non-localhost address
            ("http://120.0.0.1/", Some("<-loopback>"), false),
            // Don't bypass due to no bypass list specified
            ("http://google.com/", Some(""), false),
            // Bypass due to url matches
            ("http://google.com/", Some("google.com"), true),
            ("http://google.com/", Some("google.com,apple.com"), true),
            // Bypass due to url matches with wildcard
            ("http://google.com/", Some("*google.com"), true),
            ("http://google.com/", Some("google.com,apple.com"), true),
            // Don't bypass due to no url matches
            ("http://google.com/", Some("apple.com"), false),
            // Bypass subdomains
            ("http://my.google.com/", Some(".google.com"), true),
            // Don't bypass if subdomain not supplied
            ("http://google.com/", Some(".google.com"), false),
            // Don't bypass due to different ports even inferred
            ("http://microsoft.com/", Some("*.com:443"), false),
            // Bypass due to matching inferred ports
            ("https://microsoft.com/", Some("*.com:443"), true),
            ("https://microsoft.com/", Some("microsoft.com:443"), true),
            ("http://microsoft.com/", Some("microsoft.com:80"), true),
            // Bypass due to matching domain with any port
            ("http://microsoft.com:88/", Some("microsoft.com"), true),
            // Bypass due to matching domain with explicit port
            ("http://microsoft.com:88/", Some("microsoft.com:88"), true),
            // Bypass due to matching literal ip
            ("http://192.168.0.1/", Some("192.168.0.1"), true),
            ("http://192.168.0.1/", Some("192.168.0.1:80"), true),
            ("http://[::ffff:c0a8:1]/", Some("[::ffff:c0a8:1]"), true),
            ("http://[::ffff:c0a8:1]/", Some("[::ffff:c0a8:1]:80"), true),
            // Don't bypass due to different ip
            ("http://192.168.1.1/", Some("192.168.0.1"), false),
            // Bypass due to ip with matching cidr range
            ("http://192.167.2.1/", Some("192.168.0.1/8"), true),
            ("http://192.168.2.1/", Some("192.168.0.1/16"), true),
            ("http://192.168.0.1/", Some("192.168.0.1/24"), true),
            ("http://[::ffff:c0a8:1]/", Some("::ffff:c0a0:1/108"), true),
            ("http://[::ffff:c0a8:0]/", Some("::ffff:c0a8:1/127"), true),
            // Don't bypass due to ip without matching cidr range
            ("http://192.167.0.0/", Some("192.168.0.1/16"), false),
            ("http://192.168.1.0/", Some("192.168.0.1/24"), false),
            ("http://192.168.0.0/", Some("192.168.0.1/32"), false),
            ("http://[::ffff:c0a8:2]/", Some("::ffff:c0a8:1/127"), false),
            ("http://[::ffff:c0a8:0]/", Some("::ffff:c0a8:1/128"), false),
            // Bypass due to any rule matching, not just the first
            ("http://apple.com/", Some("google.com,apple.com"), true),
            ("http://apple.com/", Some("google.com, apple.com"), true),
            ("http://10.1.2.3/", Some("foo.com,10.0.0.0/8"), true),
            // Bypass due to an earlier rule even when a later rule's port does not match
            ("http://example.com/", Some("example.com,example.com:8080"), true),
            // Bypass due to an explicit rule even with <-loopback>
            ("http://localhost/", Some("<-loopback>,localhost"), true),
            ("http://localhost/", Some("localhost,<-loopback>"), true),
        ];
        for (url, bypass_list, expected) in tests {
            assert_eq!(
                should_bypass_proxy(url, bypass_list),
                expected,
                "url: {url} bypass list: {bypass_list:?}"
            );
        }
    }

    #[test]
    fn should_bypass_proxy_long_list() {
        let bypass_list = vec!["abcdefgh.com"; 500].join(",");
        assert!(!should_bypass_proxy("http://x.org/", Some(&bypass_list)));
        assert!(should_bypass_proxy("http://abcdefgh.com/", Some(&bypass_list)));
    }
}
