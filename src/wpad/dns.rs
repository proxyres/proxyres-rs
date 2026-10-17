//! WPAD script discovery using DNS.

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
/// The host name is removed and each parent domain is tried, stopping before the top-level domain.
pub(super) fn wpad_dns_urls(fqdn: &str) -> Vec<String> {
    let fqdn = fqdn.trim_end_matches('.');
    if fqdn.is_empty() || fqdn.eq_ignore_ascii_case("localhost") || fqdn.starts_with(|c: char| c.is_ascii_digit()) {
        return Vec::new();
    }
    let mut urls = Vec::new();
    let mut domain = fqdn;
    while let Some((_, parent)) = domain.split_once('.') {
        if !parent.contains('.') {
            break;
        }
        urls.push(format!("http://wpad.{parent}/wpad.dat"));
        domain = parent;
    }
    urls
}

/// Discovers and downloads the WPAD script using DNS, returning its url and contents.
pub(crate) fn wpad_dns(fqdn: Option<&str>) -> Option<(String, String)> {
    let fqdn = match fqdn {
        Some(fqdn) => fqdn.to_string(),
        None => local_fqdn()?,
    };
    for url in wpad_dns_urls(&fqdn) {
        log::info!("Checking next WPAD url: {url}");
        match fetch_get(&url) {
            Ok(script) => return Some((url, script)),
            Err(error) => log::info!("No WPAD script found ({error})"),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wpad_dns_url_cases() {
        let tests: [(&str, &[&str]); 7] = [
            (
                "host.corp.example.com",
                &[
                    "http://wpad.corp.example.com/wpad.dat",
                    "http://wpad.example.com/wpad.dat",
                ],
            ),
            ("host.example.com.", &["http://wpad.example.com/wpad.dat"]),
            ("host.example", &[]),
            ("host", &[]),
            ("localhost", &[]),
            ("10.0.0.1", &[]),
            ("", &[]),
        ];
        for (fqdn, expected) in tests {
            assert_eq!(wpad_dns_urls(fqdn), expected, "fqdn: {fqdn}");
        }
    }

    #[test]
    fn local_fqdn_is_available() {
        let fqdn = local_fqdn().expect("unable to get local host name");
        assert!(!fqdn.is_empty());
    }
}
