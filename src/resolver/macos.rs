//! Proxy resolver using CFNetwork.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation_sys::base::{CFEqual, CFIndex, CFRelease, CFTypeRef, kCFAllocatorDefault};
use core_foundation_sys::dictionary::CFDictionaryRef;
use core_foundation_sys::error::{CFErrorGetCode, CFErrorRef};
use core_foundation_sys::runloop::{
    CFRunLoopAddSource, CFRunLoopGetCurrent, CFRunLoopRemoveSource, CFRunLoopRunInMode, CFRunLoopSourceInvalidate,
    CFRunLoopSourceRef, CFRunLoopStop,
};
use core_foundation_sys::string::{CFStringCreateWithBytes, CFStringGetTypeID, CFStringRef, kCFStringEncodingUTF8};
use core_foundation_sys::url::{CFURLCreateWithBytes, CFURLRef};

use super::{Cancel, ResolverBackend, posix};
use crate::macos::cf::{CfOwned, cf_dictionary_i64, cf_dictionary_string, cf_dictionary_value};
use crate::macos::cf_network::{
    CFNetworkExecuteProxyAutoConfigurationScript, CFNetworkExecuteProxyAutoConfigurationURL, StreamClientContext,
    kCFProxyHostNameKey, kCFProxyPortNumberKey, kCFProxyTypeFTP, kCFProxyTypeHTTP, kCFProxyTypeHTTPS, kCFProxyTypeKey,
    kCFProxyTypeNone, kCFProxyTypeSOCKS,
};
use crate::{Error, config};

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);
const CANCEL_POLL_INTERVAL: f64 = 0.1;

// Serialize CFNetwork PAC evaluations across threads, which crash when they run concurrently.
// Chromium also serializes them with a lock.
fn pac_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

fn create_string(s: &str) -> Option<CfOwned> {
    // SAFETY: The bytes are valid for the duration of the call and the string is owned by the caller.
    CfOwned::new(unsafe {
        CFStringCreateWithBytes(
            kCFAllocatorDefault,
            s.as_ptr(),
            s.len() as CFIndex,
            kCFStringEncodingUTF8,
            0,
        )
    } as CFTypeRef)
}

fn create_url(url: &str) -> Result<CfOwned, Error> {
    // SAFETY: The bytes are valid for the duration of the call and the url is owned by the caller.
    let cf_url = unsafe {
        CFURLCreateWithBytes(
            kCFAllocatorDefault,
            url.as_ptr(),
            url.len() as CFIndex,
            kCFStringEncodingUTF8,
            std::ptr::null(),
        )
    };
    CfOwned::new(cf_url as CFTypeRef).ok_or_else(|| Error::Script(format!("invalid url {url}")))
}

// Convert an array of proxy dictionaries into a uri list
unsafe fn convert_proxy_array(proxies: CFArrayRef) -> String {
    let mut uris = Vec::new();
    // SAFETY: The caller passes a valid array of proxy dictionaries.
    unsafe {
        for i in 0..CFArrayGetCount(proxies) {
            let proxy = CFArrayGetValueAtIndex(proxies, i) as CFDictionaryRef;
            if proxy.is_null() {
                continue;
            }
            let Some(proxy_type) = cf_dictionary_value(proxy, kCFProxyTypeKey, CFStringGetTypeID() as usize) else {
                continue;
            };
            let is_type = |other: CFStringRef| CFEqual(proxy_type, other as CFTypeRef) != 0;
            let scheme = if is_type(kCFProxyTypeNone) {
                uris.push("direct://".to_string());
                continue;
            } else if is_type(kCFProxyTypeHTTP) || is_type(kCFProxyTypeHTTPS) {
                // An HTTPS proxy on macOS is an HTTP proxy that uses CONNECT for https urls
                "http"
            } else if is_type(kCFProxyTypeSOCKS) {
                "socks"
            } else if is_type(kCFProxyTypeFTP) {
                "ftp"
            } else {
                log::warn!("Unknown proxy type encountered");
                continue;
            };
            let host = cf_dictionary_string(proxy, kCFProxyHostNameKey).unwrap_or_default();
            match cf_dictionary_i64(proxy, kCFProxyPortNumberKey) {
                Some(port) => uris.push(format!("{scheme}://{host}:{port}")),
                None => uris.push(format!("{scheme}://{host}")),
            }
        }
    }
    if uris.is_empty() {
        return "direct://".to_string();
    }
    uris.join(",")
}

unsafe extern "C" fn auto_config_callback(client: *mut c_void, proxies: CFArrayRef, error: CFErrorRef) {
    // SAFETY: The client is the result cell owned by the waiting function on this thread, which
    // invalidates the run loop source before the cell goes out of scope.
    unsafe {
        let result = &*(client as *const RefCell<Option<Result<String, Error>>>);
        *result.borrow_mut() = Some(match error.is_null() {
            true => Ok(convert_proxy_array(proxies)),
            false => Err(Error::Os(CFErrorGetCode(error) as i32)),
        });
        CFRunLoopStop(CFRunLoopGetCurrent());
    }
}

// Run the PAC run loop source created with the context on the current thread's run loop
fn execute_pac_source(
    name: &str,
    cancel: &Cancel,
    create_source: impl FnOnce(*mut StreamClientContext) -> CFRunLoopSourceRef,
) -> Result<String, Error> {
    let mode = create_string("proxyres.run_loop").ok_or(Error::Os(libc::ENOMEM))?;

    let result: RefCell<Option<Result<String, Error>>> = RefCell::new(None);
    let mut context = StreamClientContext {
        version: 0,
        info: &result as *const _ as *mut c_void,
        retain: None,
        release: None,
        copy_description: None,
    };
    let _lock = pac_lock();
    let source = create_source(&mut context);
    if source.is_null() {
        return Err(Error::Script(format!("unable to execute {name}")));
    }
    // SAFETY: The context is valid and the source is invalidated before the context is dropped.
    unsafe {
        let run_loop = CFRunLoopGetCurrent();
        let mode = mode.as_ptr() as CFStringRef;
        CFRunLoopAddSource(run_loop, source, mode);
        let deadline = Instant::now() + RESOLVE_TIMEOUT;
        while result.borrow().is_none() && !cancel.is_cancelled() && Instant::now() < deadline {
            CFRunLoopRunInMode(mode, CANCEL_POLL_INTERVAL, 0);
        }
        CFRunLoopRemoveSource(run_loop, source, mode);
        CFRunLoopSourceInvalidate(source);
        CFRelease(source as CFTypeRef);
    }

    match result.into_inner() {
        Some(result) => result,
        None if cancel.is_cancelled() => Err(Error::Cancelled),
        None => Err(Error::Timeout),
    }
}

// Download and execute a PAC script url using the current thread's run loop
fn execute_pac_url(auto_config_url: &str, url: &str, cancel: &Cancel) -> Result<String, Error> {
    let pac_url = create_url(auto_config_url)?;
    let target_url = create_url(url)?;
    execute_pac_source(auto_config_url, cancel, |context| {
        // SAFETY: The urls and context outlive the source.
        unsafe {
            CFNetworkExecuteProxyAutoConfigurationURL(
                pac_url.as_ptr() as CFURLRef,
                target_url.as_ptr() as CFURLRef,
                auto_config_callback,
                context,
            )
        }
    })
}

// Execute a PAC script using the current thread's run loop
fn execute_pac_script(script: &str, url: &str, cancel: &Cancel) -> Result<String, Error> {
    let script_string = create_string(script).ok_or_else(|| Error::Script("invalid script".to_string()))?;
    let target_url = create_url(url)?;
    execute_pac_source("script", cancel, |context| {
        // SAFETY: The script, url and context outlive the source.
        unsafe {
            CFNetworkExecuteProxyAutoConfigurationScript(
                script_string.as_ptr() as CFStringRef,
                target_url.as_ptr() as CFURLRef,
                auto_config_callback,
                context,
            )
        }
    })
}

pub(super) struct MacResolver;

impl ResolverBackend for MacResolver {
    fn get_proxies_for_url(&self, url: &str, cancel: &Cancel) -> Result<String, Error> {
        if let Some(auto_config_url) = config::auto_config_url() {
            return execute_pac_url(&auto_config_url, url, cancel);
        }
        // CFNetwork does not discover PAC urls so use WPAD
        if config::auto_discover() {
            if let Some(script) = posix::discover_script(cancel)? {
                return execute_pac_script(&script, url, cancel);
            }
        }
        Ok("direct://".to_string())
    }

    fn supports_cancel(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_script() {
        let script = r#"function FindProxyForURL(url, host) { return "SOCKS socks.example.com:1080; PROXY proxy.example.com:8080; DIRECT"; }"#;
        assert_eq!(
            execute_pac_script(script, "http://example.com/", &Cancel::default()).as_deref(),
            Ok("socks://socks.example.com:1080,http://proxy.example.com:8080,direct://")
        );
    }

    #[test]
    fn execute_invalid_script() {
        let result = execute_pac_script(
            "function FindProxyForURL(url, host) {",
            "http://example.com/",
            &Cancel::default(),
        );
        assert!(result.is_err(), "{result:?}");
    }
}
