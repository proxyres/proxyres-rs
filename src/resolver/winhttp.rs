//! Proxy resolver using WinHTTP.

use std::ffi::c_void;
use std::ptr;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use windows_sys::Win32::Foundation::{ERROR_IO_PENDING, GetLastError};
use windows_sys::Win32::Networking::WinHttp::{
    ERROR_WINHTTP_AUTODETECTION_FAILED, ERROR_WINHTTP_LOGIN_FAILURE, WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
    WINHTTP_ASYNC_RESULT, WINHTTP_AUTO_DETECT_TYPE_DHCP, WINHTTP_AUTO_DETECT_TYPE_DNS_A, WINHTTP_AUTOPROXY_AUTO_DETECT,
    WINHTTP_AUTOPROXY_CONFIG_URL, WINHTTP_AUTOPROXY_OPTIONS, WINHTTP_CALLBACK_FLAG_GETPROXYFORURL_COMPLETE,
    WINHTTP_CALLBACK_FLAG_REQUEST_ERROR, WINHTTP_CALLBACK_STATUS_GETPROXYFORURL_COMPLETE,
    WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING, WINHTTP_CALLBACK_STATUS_REQUEST_ERROR, WINHTTP_FLAG_ASYNC,
    WINHTTP_INTERNET_SCHEME_FTP, WINHTTP_INTERNET_SCHEME_HTTPS, WINHTTP_INTERNET_SCHEME_SOCKS,
    WINHTTP_OPTION_CONTEXT_VALUE, WINHTTP_PROXY_RESULT, WINHTTP_STATUS_CALLBACK, WinHttpCloseHandle,
    WinHttpCreateProxyResolver, WinHttpFreeProxyResult, WinHttpGetProxyForUrlEx, WinHttpGetProxyResult, WinHttpOpen,
    WinHttpSetOption, WinHttpSetStatusCallback,
};

use super::{Cancel, ResolverBackend};
use crate::util_win::{to_wide, wide_ptr_to_string};
use crate::{Error, config};

// Notifications for handle creation and closing
const WINHTTP_CALLBACK_FLAG_HANDLES: u32 = 0x0000_0c00;

const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(100);

// Owned WinHTTP handle
struct Handle(*mut c_void);

// SAFETY: WinHTTP handles may be used and closed from any thread.
unsafe impl Send for Handle {}
// SAFETY: WinHTTP handles may be used concurrently from multiple threads.
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: The handle is owned by this wrapper.
        unsafe { WinHttpCloseHandle(self.0) };
    }
}

// Completion state shared with the status callback
#[derive(Default)]
struct CallbackState {
    result: Mutex<Option<Result<(), u32>>>,
    done: Condvar,
}

impl CallbackState {
    fn complete(&self, result: Result<(), u32>) {
        *self.result.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
        self.done.notify_all();
    }

    // Wait for the asynchronous request to complete or be cancelled
    fn wait(&self, cancel: &Cancel) -> Result<Result<(), u32>, Error> {
        let mut result = self.result.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(result) = result.take() {
                return Ok(result);
            }
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            result = self
                .done
                .wait_timeout(result, CANCEL_POLL_INTERVAL)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

unsafe extern "system" fn status_callback(
    _handle: *mut c_void,
    context: usize,
    status: u32,
    info: *mut c_void,
    _info_length: u32,
) {
    if context == 0 {
        return;
    }
    // SAFETY: The context is a reference to the callback state that is released when the handle
    // closes, which is the last notification for the handle.
    unsafe {
        let state = &*(context as *const CallbackState);
        match status {
            WINHTTP_CALLBACK_STATUS_GETPROXYFORURL_COMPLETE => state.complete(Ok(())),
            WINHTTP_CALLBACK_STATUS_REQUEST_ERROR => {
                let error = (info as *const WINHTTP_ASYNC_RESULT)
                    .as_ref()
                    .map_or(0, |result| result.dwError);
                state.complete(Err(error));
            }
            WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING => drop(Arc::from_raw(context as *const CallbackState)),
            _ => {}
        }
    }
}

// Convert a WinHTTP proxy result into a uri list
fn convert_proxy_result(result: &WINHTTP_PROXY_RESULT) -> String {
    if result.cEntries == 0 || result.pEntries.is_null() {
        return "direct://".to_string();
    }
    // SAFETY: WinHTTP returns an array of cEntries entries.
    let entries = unsafe { std::slice::from_raw_parts(result.pEntries, result.cEntries as usize) };
    entries
        .iter()
        .map(|entry| {
            if entry.fProxy == 0 {
                return "direct://".to_string();
            }
            let scheme = match entry.ProxyScheme {
                WINHTTP_INTERNET_SCHEME_HTTPS => "https",
                WINHTTP_INTERNET_SCHEME_SOCKS => "socks",
                WINHTTP_INTERNET_SCHEME_FTP => "ftp",
                _ => "http",
            };
            // SAFETY: The proxy name is a null-terminated string owned by the result.
            let host = unsafe { wide_ptr_to_string(entry.pwszProxy) };
            match entry.ProxyPort {
                0 => format!("{scheme}://{host}"),
                port => format!("{scheme}://{host}:{port}"),
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) struct WinHttpResolver {
    session: Handle,
}

impl WinHttpResolver {
    pub(super) fn new() -> Option<Self> {
        let agent = to_wide("proxyres");
        // SAFETY: The agent string is null-terminated and outlives the call.
        let session = unsafe {
            WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
                ptr::null(),
                ptr::null(),
                WINHTTP_FLAG_ASYNC,
            )
        };
        if session.is_null() {
            // SAFETY: GetLastError has no preconditions.
            log::error!("Unable to open WinHTTP session ({})", unsafe { GetLastError() });
            return None;
        }
        Some(WinHttpResolver {
            session: Handle(session),
        })
    }

    // Run a single asynchronous proxy resolution request
    fn request(&self, url: &[u16], options: &WINHTTP_AUTOPROXY_OPTIONS, cancel: &Cancel) -> Result<String, Error> {
        let mut resolver = ptr::null_mut();
        // SAFETY: The session handle is valid and the resolver handle is owned below.
        let error = unsafe { WinHttpCreateProxyResolver(self.session.0, &mut resolver) };
        if error != 0 {
            return Err(Error::Os(error as i32));
        }
        let resolver = Handle(resolver);

        // WinHTTP holds a reference to the state until the handle closing notification
        let state = Arc::new(CallbackState::default());
        let context = Arc::into_raw(state.clone()) as usize;
        // SAFETY: The handle is valid and the context outlives every notification for the handle.
        unsafe {
            let installed = WinHttpSetOption(
                resolver.0,
                WINHTTP_OPTION_CONTEXT_VALUE,
                &context as *const usize as *const c_void,
                size_of::<usize>() as u32,
            ) != 0
                && std::mem::transmute::<WINHTTP_STATUS_CALLBACK, usize>(WinHttpSetStatusCallback(
                    resolver.0,
                    Some(status_callback),
                    WINHTTP_CALLBACK_FLAG_REQUEST_ERROR
                        | WINHTTP_CALLBACK_FLAG_GETPROXYFORURL_COMPLETE
                        | WINHTTP_CALLBACK_FLAG_HANDLES,
                    0,
                )) != usize::MAX;
            if !installed {
                let error = GetLastError();
                // Without the callback no notification releases the reference
                drop(Arc::from_raw(context as *const CallbackState));
                return Err(Error::Os(error as i32));
            }
        }

        // SAFETY: The url and options are valid for the duration of the call.
        let error = unsafe { WinHttpGetProxyForUrlEx(resolver.0, url.as_ptr(), options, context) };
        match error {
            ERROR_IO_PENDING => {}
            ERROR_WINHTTP_AUTODETECTION_FAILED => return Ok("direct://".to_string()),
            error => return Err(Error::Os(error as i32)),
        }

        match state.wait(cancel)? {
            Ok(()) => {}
            // Failed to detect proxy auto configuration url so use direct connection
            Err(ERROR_WINHTTP_AUTODETECTION_FAILED) => return Ok("direct://".to_string()),
            Err(error) => return Err(Error::Os(error as i32)),
        }

        let mut result = WINHTTP_PROXY_RESULT {
            cEntries: 0,
            pEntries: ptr::null_mut(),
        };
        // SAFETY: The request completed and the result is freed below.
        let error = unsafe { WinHttpGetProxyResult(resolver.0, &mut result) };
        if error != 0 {
            return Err(Error::Os(error as i32));
        }
        let list = convert_proxy_result(&result);
        // SAFETY: The result was populated by WinHttpGetProxyResult.
        unsafe { WinHttpFreeProxyResult(&mut result) };
        Ok(list)
    }
}

impl ResolverBackend for WinHttpResolver {
    fn get_proxies_for_url(&self, url: &str, cancel: &Cancel) -> Result<String, Error> {
        let auto_config_url = config::auto_config_url().map(|url| to_wide(&url));
        let mut options = WINHTTP_AUTOPROXY_OPTIONS {
            dwFlags: 0,
            dwAutoDetectFlags: 0,
            lpszAutoConfigUrl: ptr::null(),
            lpvReserved: ptr::null_mut(),
            dwReserved: 0,
            fAutoLogonIfChallenged: 0,
        };
        match &auto_config_url {
            // Use auto configuration script specified by system
            Some(auto_config_url) => {
                options.dwFlags = WINHTTP_AUTOPROXY_CONFIG_URL;
                options.lpszAutoConfigUrl = auto_config_url.as_ptr();
            }
            // Use WPAD to automatically retrieve proxy auto-configuration and evaluate it
            None => {
                options.dwFlags = WINHTTP_AUTOPROXY_AUTO_DETECT;
                options.dwAutoDetectFlags = WINHTTP_AUTO_DETECT_TYPE_DHCP | WINHTTP_AUTO_DETECT_TYPE_DNS_A;
            }
        }
        let url = to_wide(url);

        // For performance reasons try without automatic logon and then with it
        // https://docs.microsoft.com/en-us/windows/win32/api/winhttp/ns-winhttp-winhttp_autoproxy_options
        match self.request(&url, &options, cancel) {
            Err(Error::Os(error)) if error == ERROR_WINHTTP_LOGIN_FAILURE as i32 => {
                options.fAutoLogonIfChallenged = 1;
                self.request(&url, &options, cancel)
            }
            result => result,
        }
    }

    fn supports_cancel(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::Networking::WinHttp::{WINHTTP_INTERNET_SCHEME_HTTP, WINHTTP_PROXY_RESULT_ENTRY};

    use super::*;

    fn entry(scheme: i32, host: &mut [u16], port: u16) -> WINHTTP_PROXY_RESULT_ENTRY {
        WINHTTP_PROXY_RESULT_ENTRY {
            fProxy: 1,
            fBypass: 0,
            ProxyScheme: scheme,
            pwszProxy: host.as_mut_ptr(),
            ProxyPort: port,
        }
    }

    #[test]
    fn convert_entries() {
        let (mut socks, mut secure, mut files, mut plain) =
            (to_wide("socks"), to_wide("secure"), to_wide("files"), to_wide("plain"));
        let mut entries = [
            entry(WINHTTP_INTERNET_SCHEME_SOCKS, &mut socks, 1080),
            entry(WINHTTP_INTERNET_SCHEME_HTTPS, &mut secure, 443),
            entry(WINHTTP_INTERNET_SCHEME_FTP, &mut files, 21),
            // Entries without a port leave it out
            entry(WINHTTP_INTERNET_SCHEME_HTTP, &mut plain, 0),
            // Entries that are not proxies are direct connections
            WINHTTP_PROXY_RESULT_ENTRY::default(),
        ];
        let result = WINHTTP_PROXY_RESULT {
            cEntries: entries.len() as u32,
            pEntries: entries.as_mut_ptr(),
        };
        assert_eq!(
            convert_proxy_result(&result),
            "socks://socks:1080,https://secure:443,ftp://files:21,http://plain,direct://"
        );
    }

    #[test]
    fn convert_empty_result() {
        let result = WINHTTP_PROXY_RESULT {
            cEntries: 0,
            pEntries: ptr::null_mut(),
        };
        assert_eq!(convert_proxy_result(&result), "direct://");
    }
}
