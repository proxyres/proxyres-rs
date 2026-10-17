//! C API compatible with the headers of the proxyres C library.
//!
//! Strings returned to the caller for ownership are allocated with `malloc` and must be released
//! with `free`. Strings returned as `const char *` remain owned by the instance and are valid until
//! the next call on that instance or until it is deleted.

use std::ffi::{CStr, CString, c_char, c_void};
use std::time::Duration;

use proxyres::resolver::Resolver;
use proxyres::{Error, config, execute};

/// Copies a string into memory allocated with `malloc` so the caller can release it with `free`.
fn to_malloc_string(s: Option<String>) -> *mut c_char {
    let Some(s) = s else {
        return std::ptr::null_mut();
    };
    // C strings end at the first null character
    let bytes = s.as_bytes();
    let bytes = &bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())];
    // SAFETY: The allocation is large enough for the bytes and the null terminator.
    unsafe {
        let ptr = libc::malloc(bytes.len() + 1) as *mut u8;
        if ptr.is_null() {
            return std::ptr::null_mut();
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        *ptr.add(bytes.len()) = 0;
        ptr as *mut c_char
    }
}

/// Converts a string into a C string owned by an instance.
fn to_c_string(s: &str) -> CString {
    let bytes = s.as_bytes();
    let bytes = &bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())];
    CString::new(bytes).unwrap_or_default()
}

/// Reads a null-terminated UTF-8 string from the caller.
///
/// # Safety
///
/// The pointer must be null or point to a null-terminated string.
unsafe fn from_c_str<'a>(s: *const c_char) -> Option<std::borrow::Cow<'a, str>> {
    if s.is_null() {
        return None;
    }
    // SAFETY: The caller guarantees the string is null-terminated.
    Some(unsafe { CStr::from_ptr(s) }.to_string_lossy())
}

/// Maps an error to the error codes reported by the C library.
fn error_code(error: &Error) -> i32 {
    match error {
        Error::Os(code) => *code,
        Error::Cancelled => libc::ECANCELED,
        Error::Timeout => libc::ETIMEDOUT,
        Error::Fetch(_) => libc::EIO,
        Error::NoResolver | Error::NoScriptEngine => libc::ENOSYS,
        _ => libc::EINVAL,
    }
}

// Library

/// Initializes the proxy resolution library.
#[unsafe(no_mangle)]
pub extern "C" fn proxyres_global_init() -> bool {
    true
}

/// Uninitializes the proxy resolution library.
#[unsafe(no_mangle)]
pub extern "C" fn proxyres_global_cleanup() -> bool {
    true
}

// Config

/// Reads whether WPAD is enabled on the user's system.
#[unsafe(no_mangle)]
pub extern "C" fn proxy_config_get_auto_discover() -> bool {
    config::auto_discover()
}

/// Reads the proxy auto config (PAC) url configured on the user's system.
#[unsafe(no_mangle)]
pub extern "C" fn proxy_config_get_auto_config_url() -> *mut c_char {
    to_malloc_string(config::auto_config_url())
}

/// Reads the proxy configured on the user's system for a given URL scheme.
///
/// # Safety
///
/// The scheme must be null or a null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proxy_config_get_proxy(scheme: *const c_char) -> *mut c_char {
    // SAFETY: The caller guarantees the scheme is a valid string.
    let Some(scheme) = (unsafe { from_c_str(scheme) }) else {
        return std::ptr::null_mut();
    };
    to_malloc_string(config::proxy(&scheme))
}

/// Reads the proxy bypass list configured on the user's system.
#[unsafe(no_mangle)]
pub extern "C" fn proxy_config_get_bypass_list() -> *mut c_char {
    to_malloc_string(config::bypass_list())
}

/// Overrides the user's configured proxy auto configuration (PAC) url.
///
/// # Safety
///
/// The url must be null or a null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proxy_config_set_auto_config_url_override(auto_config_url: *const c_char) {
    // SAFETY: The caller guarantees the url is a valid string.
    config::set_auto_config_url_override(unsafe { from_c_str(auto_config_url) }.as_deref());
}

/// Overrides the user's configured proxy.
///
/// # Safety
///
/// The proxy must be null or a null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proxy_config_set_proxy_override(proxy: *const c_char) {
    // SAFETY: The caller guarantees the proxy is a valid string.
    config::set_proxy_override(unsafe { from_c_str(proxy) }.as_deref());
}

/// Overrides the user's configured proxy bypass list.
///
/// # Safety
///
/// The bypass list must be null or a null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proxy_config_set_bypass_list_override(bypass_list: *const c_char) {
    // SAFETY: The caller guarantees the bypass list is a valid string.
    config::set_bypass_list_override(unsafe { from_c_str(bypass_list) }.as_deref());
}

/// Initialization function for reading user's proxy configuration.
#[unsafe(no_mangle)]
pub extern "C" fn proxy_config_global_init() -> bool {
    true
}

/// Uninitialization function for reading user's proxy configuration.
#[unsafe(no_mangle)]
pub extern "C" fn proxy_config_global_cleanup() -> bool {
    true
}

// Resolver

struct ProxyResolver {
    resolver: Resolver,
    list: Option<CString>,
}

/// Gets the resolver instance from a context pointer.
///
/// # Safety
///
/// The pointer must be null or a pointer returned by `proxy_resolver_create`.
unsafe fn resolver<'a>(ctx: *mut c_void) -> Option<&'a mut ProxyResolver> {
    // SAFETY: The caller guarantees the pointer was created by proxy_resolver_create.
    unsafe { (ctx as *mut ProxyResolver).as_mut() }
}

/// Asynchronously resolves the proxies for a given URL based on the user's proxy configuration.
///
/// # Safety
///
/// The context must be a resolver instance and the url a null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proxy_resolver_get_proxies_for_url(ctx: *mut c_void, url: *const c_char) -> bool {
    // SAFETY: The caller guarantees the arguments are valid.
    let (Some(resolver), Some(url)) = (unsafe { resolver(ctx) }, unsafe { from_c_str(url) }) else {
        return false;
    };
    resolver.list = None;
    resolver.resolver.get_proxies_for_url(&url).is_ok()
}

/// Gets the list of proxies that have been resolved.
///
/// # Safety
///
/// The context must be a resolver instance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proxy_resolver_get_list(ctx: *mut c_void) -> *const c_char {
    // SAFETY: The caller guarantees the context is valid.
    let Some(resolver) = (unsafe { resolver(ctx) }) else {
        return std::ptr::null();
    };
    let Some(list) = resolver.resolver.list() else {
        return std::ptr::null();
    };
    resolver.list.get_or_insert_with(|| to_c_string(list)).as_ptr()
}

/// Gets the next proxy in the list of proxies that have been resolved.
///
/// # Safety
///
/// The context must be a resolver instance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proxy_resolver_get_next_proxy(ctx: *mut c_void) -> *mut c_char {
    // SAFETY: The caller guarantees the context is valid.
    let Some(resolver) = (unsafe { resolver(ctx) }) else {
        return std::ptr::null_mut();
    };
    to_malloc_string(resolver.resolver.next_proxy())
}

/// Error code for proxy resolution process.
///
/// # Safety
///
/// The context must be a resolver instance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proxy_resolver_get_error(ctx: *mut c_void) -> i32 {
    // SAFETY: The caller guarantees the context is valid.
    let Some(resolver) = (unsafe { resolver(ctx) }) else {
        return -1;
    };
    resolver.resolver.error().map_or(0, error_code)
}

/// Waits for the proxy resolution process to complete, or indefinitely if the timeout is negative.
///
/// # Safety
///
/// The context must be a resolver instance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proxy_resolver_wait(ctx: *mut c_void, timeout_ms: i32) -> bool {
    // SAFETY: The caller guarantees the context is valid.
    let Some(resolver) = (unsafe { resolver(ctx) }) else {
        return false;
    };
    let timeout = u64::try_from(timeout_ms).ok().map(Duration::from_millis);
    resolver.resolver.wait(timeout)
}

/// Cancel any pending proxy resolution.
///
/// # Safety
///
/// The context must be a resolver instance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proxy_resolver_cancel(ctx: *mut c_void) -> bool {
    // SAFETY: The caller guarantees the context is valid.
    let Some(resolver) = (unsafe { resolver(ctx) }) else {
        return false;
    };
    resolver.resolver.cancel()
}

/// Create a proxy resolver instance.
#[unsafe(no_mangle)]
pub extern "C" fn proxy_resolver_create() -> *mut c_void {
    Box::into_raw(Box::new(ProxyResolver {
        resolver: Resolver::new(),
        list: None,
    })) as *mut c_void
}

/// Deletes a proxy resolver instance.
///
/// # Safety
///
/// The pointer must be null or point to null or a resolver instance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proxy_resolver_delete(ctx: *mut *mut c_void) -> bool {
    // SAFETY: The caller guarantees the pointer is valid and the instance was created by
    // proxy_resolver_create.
    unsafe {
        if ctx.is_null() || (*ctx).is_null() {
            return false;
        }
        drop(Box::from_raw(*ctx as *mut ProxyResolver));
        *ctx = std::ptr::null_mut();
    }
    true
}

/// Initialization function for proxy resolution.
#[unsafe(no_mangle)]
pub extern "C" fn proxy_resolver_global_init() -> bool {
    true
}

/// Uninitialization function for proxy resolution.
#[unsafe(no_mangle)]
pub extern "C" fn proxy_resolver_global_cleanup() -> bool {
    true
}

// Execute

struct ProxyExecute {
    list: Option<CString>,
    error: i32,
}

/// Gets the execute instance from a context pointer.
///
/// # Safety
///
/// The pointer must be null or a pointer returned by `proxy_execute_create`.
unsafe fn executor<'a>(ctx: *mut c_void) -> Option<&'a mut ProxyExecute> {
    // SAFETY: The caller guarantees the pointer was created by proxy_execute_create.
    unsafe { (ctx as *mut ProxyExecute).as_mut() }
}

/// Executes a PAC script for a particular URL.
///
/// # Safety
///
/// The context must be an execute instance and the script and url null-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proxy_execute_get_proxies_for_url(
    ctx: *mut c_void,
    script: *const c_char,
    url: *const c_char,
) -> bool {
    // SAFETY: The caller guarantees the arguments are valid.
    let (Some(executor), Some(script), Some(url)) = (unsafe { executor(ctx) }, unsafe { from_c_str(script) }, unsafe {
        from_c_str(url)
    }) else {
        return false;
    };
    match execute::get_proxies_for_url(&script, &url) {
        Ok(list) => {
            executor.list = Some(to_c_string(&list));
            executor.error = 0;
            true
        }
        Err(error) => {
            executor.list = None;
            executor.error = error_code(&error);
            false
        }
    }
}

/// Get the list of proxies returned by the call to `FindProxyForURL`.
///
/// # Safety
///
/// The context must be an execute instance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proxy_execute_get_list(ctx: *mut c_void) -> *const c_char {
    // SAFETY: The caller guarantees the context is valid.
    unsafe { executor(ctx) }
        .and_then(|executor| executor.list.as_ref())
        .map_or(std::ptr::null(), |list| list.as_ptr())
}

/// Error code for script execution.
///
/// # Safety
///
/// The context must be an execute instance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proxy_execute_get_error(ctx: *mut c_void) -> i32 {
    // SAFETY: The caller guarantees the context is valid.
    unsafe { executor(ctx) }.map_or(-1, |executor| executor.error)
}

/// Create new PAC script execution instance.
#[unsafe(no_mangle)]
pub extern "C" fn proxy_execute_create() -> *mut c_void {
    Box::into_raw(Box::new(ProxyExecute { list: None, error: 0 })) as *mut c_void
}

/// Delete a PAC script execution instance.
///
/// # Safety
///
/// The pointer must be null or point to null or an execute instance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proxy_execute_delete(ctx: *mut *mut c_void) -> bool {
    // SAFETY: The caller guarantees the pointer is valid and the instance was created by
    // proxy_execute_create.
    unsafe {
        if ctx.is_null() || (*ctx).is_null() {
            return false;
        }
        drop(Box::from_raw(*ctx as *mut ProxyExecute));
        *ctx = std::ptr::null_mut();
    }
    true
}

/// Initialization function for PAC script execution.
#[unsafe(no_mangle)]
pub extern "C" fn proxy_execute_global_init() -> bool {
    execute::is_available()
}

/// Uninitialization function for PAC script execution.
#[unsafe(no_mangle)]
pub extern "C" fn proxy_execute_global_cleanup() -> bool {
    true
}
