//! WPAD script discovery using DNS.

use crate::Error;
use crate::fetch::fetch_get;

/// Gets the fully qualified domain name of the local host.
#[cfg(windows)]
fn local_fqdn() -> Option<String> {
    use windows_sys::Win32::System::SystemInformation::{ComputerNameDnsFullyQualified, GetComputerNameExW};

    let mut size: u32 = 0;
    // SAFETY: The first call only queries the required buffer size.
    unsafe { GetComputerNameExW(ComputerNameDnsFullyQualified, std::ptr::null_mut(), &mut size) };
    let mut buffer = vec![0u16; size as usize + 1];
    size = buffer.len() as u32;
    // SAFETY: The buffer holds `size` wide characters.
    if unsafe { GetComputerNameExW(ComputerNameDnsFullyQualified, buffer.as_mut_ptr(), &mut size) } == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..size as usize]))
}

/// Gets the fully qualified domain name of the local host.
#[cfg(unix)]
fn local_fqdn() -> Option<String> {
    use std::ffi::CStr;

    let mut hostname = [0 as libc::c_char; 256];
    // SAFETY: The buffer is large enough for the host name and is null-terminated below.
    if unsafe { libc::gethostname(hostname.as_mut_ptr(), hostname.len() - 1) } != 0 {
        log::error!("Unable to get hostname ({})", std::io::Error::last_os_error());
        return None;
    }
    // SAFETY: The buffer is zero-initialized and the last byte is never written.
    let hostname = unsafe { CStr::from_ptr(hostname.as_ptr()) };

    // Canonical name is returned in the first address
    // SAFETY: The hints are zero-initialized and the result list is freed before returning.
    unsafe {
        let mut hints: libc::addrinfo = std::mem::zeroed();
        hints.ai_flags = libc::AI_CANONNAME;
        hints.ai_family = libc::AF_UNSPEC;
        hints.ai_socktype = libc::SOCK_STREAM;
        let mut result: *mut libc::addrinfo = std::ptr::null_mut();
        if libc::getaddrinfo(hostname.as_ptr(), std::ptr::null(), &hints, &mut result) == 0 {
            let canonical_name = result
                .as_ref()
                .filter(|info| !info.ai_canonname.is_null())
                .map(|info| CStr::from_ptr(info.ai_canonname).to_string_lossy().into_owned());
            libc::freeaddrinfo(result);
            if canonical_name.is_some() {
                return canonical_name;
            }
        }
    }
    Some(hostname.to_string_lossy().into_owned())
}

/// Gets the WPAD urls to try for a fully qualified domain name.
///
/// Each suffix of the name with at least two labels is tried, starting with the full name. Empty
/// labels are ignored.
pub(super) fn wpad_dns_urls(fqdn: &str) -> Vec<String> {
    let labels: Vec<&str> = fqdn.split('.').filter(|label| !label.is_empty()).collect();
    (0..labels.len().saturating_sub(1))
        .map(|i| format!("http://wpad.{}/wpad.dat", labels[i..].join(".")))
        .collect()
}

// Check whether the local host name can be used for WPAD discovery
fn is_valid_fqdn(fqdn: &str) -> bool {
    !fqdn.is_empty() && !fqdn.eq_ignore_ascii_case("localhost") && !fqdn.starts_with(|c: char| c.is_ascii_digit())
}

/// Discovers and downloads the WPAD script using DNS, returning its url and contents.
pub(crate) fn wpad_dns(fqdn: Option<&str>) -> Option<(String, String)> {
    wpad_dns_with_fetch(fqdn, fetch_get)
}

fn wpad_dns_with_fetch(fqdn: Option<&str>, fetch: impl Fn(&str) -> Result<String, Error>) -> Option<(String, String)> {
    let fqdn = match fqdn {
        Some(fqdn) => fqdn.to_string(),
        None => local_fqdn().filter(|fqdn| is_valid_fqdn(fqdn))?,
    };
    for url in wpad_dns_urls(&fqdn) {
        log::info!("Checking next WPAD url: {url}");
        match fetch(&url) {
            Ok(script) => return Some((url, script)),
            Err(error) => log::info!("No WPAD script found ({error})"),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn wpad_dns_url_cases() {
        let tests: [(&str, &[&str]); 10] = [
            (
                "host.sub.example.com",
                &[
                    "http://wpad.host.sub.example.com/wpad.dat",
                    "http://wpad.sub.example.com/wpad.dat",
                    "http://wpad.example.com/wpad.dat",
                ],
            ),
            ("example.com", &["http://wpad.example.com/wpad.dat"]),
            ("localhost", &[]),
            ("", &[]),
            (
                "host.example.com.",
                &[
                    "http://wpad.host.example.com/wpad.dat",
                    "http://wpad.example.com/wpad.dat",
                ],
            ),
            // Leading dot is skipped
            (".example.com", &["http://wpad.example.com/wpad.dat"]),
            // All empty labels
            ("...", &[]),
            (".", &[]),
            (
                "a.b.c.d.e.com",
                &[
                    "http://wpad.a.b.c.d.e.com/wpad.dat",
                    "http://wpad.b.c.d.e.com/wpad.dat",
                    "http://wpad.c.d.e.com/wpad.dat",
                    "http://wpad.d.e.com/wpad.dat",
                    "http://wpad.e.com/wpad.dat",
                ],
            ),
            // Consecutive dots are collapsed
            (
                "host..example.com",
                &[
                    "http://wpad.host.example.com/wpad.dat",
                    "http://wpad.example.com/wpad.dat",
                ],
            ),
        ];
        for (fqdn, expected) in tests {
            assert_eq!(wpad_dns_urls(fqdn), expected, "fqdn: {fqdn}");
        }
    }

    #[test]
    fn valid_fqdn() {
        assert!(is_valid_fqdn("host.example.com"));
        assert!(!is_valid_fqdn("localhost"));
        assert!(!is_valid_fqdn("10.0.0.1"));
        assert!(!is_valid_fqdn(""));
    }

    fn not_found(url: &str) -> Error {
        Error::Fetch(format!("{url}: not found"))
    }

    #[test]
    fn fetch_returns_script() {
        let target = "http://wpad.host.example.com/wpad.dat";
        let script = "function FindProxyForURL() { return \"PROXY proxy:8080\"; }";
        let fetch = |url: &str| {
            if url == target {
                Ok(script.to_string())
            } else {
                Err(not_found(url))
            }
        };
        let result = wpad_dns_with_fetch(Some("host.example.com"), fetch);
        assert_eq!(result, Some((target.to_string(), script.to_string())));
    }

    #[test]
    fn fetch_all_fail() {
        assert_eq!(
            wpad_dns_with_fetch(Some("host.example.com"), |url| Err(not_found(url))),
            None
        );
    }

    #[test]
    fn fetch_first_url_succeeds() {
        let calls = Cell::new(0);
        let fetch = |_: &str| {
            calls.set(calls.get() + 1);
            Ok("function FindProxyForURL() { return \"DIRECT\"; }".to_string())
        };
        assert!(wpad_dns_with_fetch(Some("host.sub.example.com"), fetch).is_some());
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn local_fqdn_is_available() {
        let fqdn = local_fqdn().expect("unable to get local host name");
        assert!(!fqdn.is_empty());
    }
}
