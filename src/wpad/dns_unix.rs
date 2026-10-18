//! Local host name for WPAD discovery using DNS on Unix.

use std::ffi::CStr;

/// Gets the fully qualified domain name of the local host.
pub(super) fn local_fqdn() -> Option<String> {
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
