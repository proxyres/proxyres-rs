//! Windows string and WinHTTP proxy list helpers.

use crate::util::url_scheme;

/// Converts a string into a null-terminated wide string.
pub(crate) fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Converts a null-terminated wide string into a string.
///
/// # Safety
///
/// The pointer must be null or point to a null-terminated wide string.
pub(crate) unsafe fn wide_ptr_to_string(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: The caller guarantees the string is null-terminated.
    unsafe {
        let len = (0..).take_while(|&i| *ptr.add(i) != 0).count();
        String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
    }
}

// Find the end of the scheme in a WinHTTP proxy list entry
fn winhttp_scheme_end(config: &str) -> Option<usize> {
    let equals = config.find('=');
    let separator = config.find("://");
    match (equals, separator) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

// Split a WinHTTP proxy list entry into its optional scheme and host
fn split_winhttp_proxy(config: &str) -> (Option<&str>, &str) {
    match winhttp_scheme_end(config) {
        Some(end) => (Some(&config[..end]), config[end..].trim_start_matches(['=', ':', '/'])),
        None => (None, config),
    }
}

// Enumerate the entries in a WinHTTP proxy list separated by semicolons or whitespace
fn winhttp_proxy_entries(proxy_list: &str) -> impl Iterator<Item = &str> {
    proxy_list
        .split(|c: char| c == ';' || c.is_ascii_whitespace())
        .filter(|config| !config.is_empty())
}

/// Gets the proxy for a scheme from a WinHTTP proxy list.
///
/// The proxy list contains one or more entries separated by semicolons or whitespace:
///
/// ```text
/// ([<scheme>=][<scheme>"://"]<server>[":"<port>])
/// ```
///
/// Entries without a scheme are used for any scheme that has no entry of its own.
pub(crate) fn winhttp_proxy_by_scheme(scheme: &str, proxy_list: &str) -> Option<String> {
    let scheme = url_scheme(scheme).unwrap_or(scheme);
    let mut fallback = None;
    for config in winhttp_proxy_entries(proxy_list) {
        match split_winhttp_proxy(config) {
            (Some(config_scheme), host) if config_scheme.eq_ignore_ascii_case(scheme) => {
                return Some(host.to_string());
            }
            (None, host) if fallback.is_none() => fallback = Some(host),
            _ => {}
        }
    }
    fallback.map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let wide = to_wide("proxy:80");
        assert_eq!(wide.last(), Some(&0));
        assert_eq!(unsafe { wide_ptr_to_string(wide.as_ptr()) }, "proxy:80");
        assert_eq!(unsafe { wide_ptr_to_string(std::ptr::null()) }, "");
    }

    #[test]
    fn winhttp_proxy_by_scheme_cases() {
        let tests = [
            // No schemes in list
            ("1proxy.com", "http", Some("1proxy.com")),
            ("2proxy.com", "https", Some("2proxy.com")),
            // One http scheme in list
            ("http=3proxy.com", "http", Some("3proxy.com")),
            ("http=4proxy.com:80", "http", Some("4proxy.com:80")),
            // One https scheme in list
            ("https=5proxy.com", "https", Some("5proxy.com")),
            ("https=6proxy.com:443", "https", Some("6proxy.com:443")),
            // Two schemes in list
            ("http=7proxy.com;https=8proxy.com", "http", Some("7proxy.com")),
            ("https=9proxy.com;http=10proxy.com", "http", Some("10proxy.com")),
            // Two schemes in proxy list reversed
            (
                "http=11proxy.com;https=12proxy.com:443",
                "https",
                Some("12proxy.com:443"),
            ),
            ("https=13proxy.com;http=14proxy.com:80;", "https", Some("13proxy.com")),
            // Whitespace after a separator is not an entry
            ("ftp=x:21; http=y:80", "http", Some("y:80")),
            // No entry for the scheme
            ("http=a:80;ftp=c:21", "https", None),
            // Entry without a scheme is used when the scheme has no entry
            ("http=a:80;b:8080", "https", Some("b:8080")),
            // Scheme of a url is used
            ("http=a:80;https=b:443", "https://example.com/", Some("b:443")),
        ];
        for (proxy_list, scheme, expected) in tests {
            assert_eq!(
                winhttp_proxy_by_scheme(scheme, proxy_list).as_deref(),
                expected,
                "proxy list: {proxy_list} scheme: {scheme}"
            );
        }
    }
}
