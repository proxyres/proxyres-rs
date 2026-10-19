use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::{Mutex, MutexGuard, PoisonError};

use proxyres_capi::*;

const PAC_SCRIPT: &str = include_str!("../../tests/pac.js");

// Serializes tests that log messages or read the last logged message
fn log_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

// Take ownership of a string allocated with malloc
fn take_string(ptr: *mut c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: The string was allocated with malloc by the library.
    unsafe {
        let string = CStr::from_ptr(ptr).to_string_lossy().into_owned();
        libc::free(ptr as *mut c_void);
        Some(string)
    }
}

fn borrowed_string(ptr: *const c_char) -> Option<String> {
    // SAFETY: The string is owned by the instance and valid until the next call.
    (!ptr.is_null()).then(|| unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned())
}

#[test]
fn global_init() {
    assert!(proxyres_global_init());
    assert!(proxy_config_global_init());
    assert!(proxy_resolver_global_init());
    assert!(proxyres_global_cleanup());
    assert!(proxy_config_global_cleanup());
    assert!(proxy_resolver_global_cleanup());
    assert!(proxy_execute_global_cleanup());
}

// Config overrides are global so all config and resolver checks run in one test
#[test]
fn config_and_resolver() {
    let _lock = log_lock();
    let proxy = CString::new("my.proxy:8080").unwrap();
    let bypass_list = CString::new("<local>,*.example.com").unwrap();
    let scheme = CString::new("https").unwrap();
    let auto_config_url = CString::new("http://127.0.0.1:8080/pac.js").unwrap();
    unsafe {
        proxy_config_set_auto_config_url_override(auto_config_url.as_ptr());
        assert_eq!(
            take_string(proxy_config_get_auto_config_url()).as_deref(),
            Some("http://127.0.0.1:8080/pac.js")
        );
        proxy_config_set_auto_config_url_override(std::ptr::null());

        proxy_config_set_proxy_override(proxy.as_ptr());
        proxy_config_set_bypass_list_override(bypass_list.as_ptr());
        assert_eq!(
            take_string(proxy_config_get_proxy(scheme.as_ptr())).as_deref(),
            Some("my.proxy:8080")
        );
        assert_eq!(
            take_string(proxy_config_get_bypass_list()).as_deref(),
            Some("<local>,*.example.com")
        );
        assert_eq!(take_string(proxy_config_get_proxy(std::ptr::null())), None);
        assert!(!proxy_config_get_auto_discover());

        let mut resolver = proxy_resolver_create();
        assert!(!resolver.is_null());
        assert!(proxy_resolver_get_list(resolver).is_null());
        let url = CString::new("https://example.org/").unwrap();
        assert!(proxy_resolver_get_proxies_for_url(resolver, url.as_ptr()));
        assert!(proxy_resolver_wait(resolver, -1));
        assert_eq!(
            borrowed_string(proxy_resolver_get_list(resolver)).as_deref(),
            Some("http://my.proxy:8080")
        );
        assert_eq!(
            take_string(proxy_resolver_get_next_proxy(resolver)).as_deref(),
            Some("http://my.proxy:8080")
        );
        assert_eq!(take_string(proxy_resolver_get_next_proxy(resolver)), None);
        assert_eq!(proxy_resolver_get_error(resolver), 0);

        // Bypassed url uses a direct connection
        let url = CString::new("https://www.example.com/").unwrap();
        assert!(proxy_resolver_get_proxies_for_url(resolver, url.as_ptr()));
        assert!(proxy_resolver_wait(resolver, 0));
        assert_eq!(
            borrowed_string(proxy_resolver_get_list(resolver)).as_deref(),
            Some("direct://")
        );
        // Cancelling a completed resolution succeeds
        assert!(proxy_resolver_cancel(resolver));

        assert!(proxy_resolver_delete(&mut resolver));
        assert!(resolver.is_null());
        assert!(!proxy_resolver_delete(&mut resolver));

        proxy_config_set_proxy_override(std::ptr::null());
        proxy_config_set_bypass_list_override(std::ptr::null());
    }
}

#[test]
fn null_instances() {
    unsafe {
        assert!(!proxy_resolver_get_proxies_for_url(
            std::ptr::null_mut(),
            std::ptr::null()
        ));
        assert!(proxy_resolver_get_list(std::ptr::null_mut()).is_null());
        assert!(proxy_resolver_get_next_proxy(std::ptr::null_mut()).is_null());
        assert_eq!(proxy_resolver_get_error(std::ptr::null_mut()), -1);
        assert!(!proxy_resolver_wait(std::ptr::null_mut(), 0));
        assert!(!proxy_resolver_cancel(std::ptr::null_mut()));
        assert!(!proxy_resolver_delete(std::ptr::null_mut()));
        assert!(!proxy_execute_get_proxies_for_url(
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null()
        ));
        assert!(proxy_execute_get_list(std::ptr::null_mut()).is_null());
        assert_eq!(proxy_execute_get_error(std::ptr::null_mut()), -1);
        assert!(!proxy_execute_delete(std::ptr::null_mut()));
    }
}

#[test]
fn execute() {
    if !proxy_execute_global_init() {
        return;
    }
    let script = CString::new(PAC_SCRIPT).unwrap();
    unsafe {
        let mut execute = proxy_execute_create();
        let url = CString::new("http://simple.com/").unwrap();
        assert!(proxy_execute_get_proxies_for_url(
            execute,
            script.as_ptr(),
            url.as_ptr()
        ));
        assert_eq!(proxy_execute_get_error(execute), 0);
        assert_eq!(
            borrowed_string(proxy_execute_get_list(execute)).as_deref(),
            Some("PROXY no-such-proxy:80")
        );

        let bad_scripts = [
            "function FindProxyForURL(url, host) {",
            // Thrown value that is not an Error
            "function FindProxyForURL(url, host) { throw 42; }",
            // Undefined function
            "function FindProxyForURL(url, host) { return notDefined(host); }",
        ];
        for bad_script in bad_scripts {
            let bad_script = CString::new(bad_script).unwrap();
            assert!(!proxy_execute_get_proxies_for_url(
                execute,
                bad_script.as_ptr(),
                url.as_ptr()
            ));
            assert_ne!(proxy_execute_get_error(execute), 0);
            assert!(proxy_execute_get_list(execute).is_null());
        }

        assert!(proxy_execute_delete(&mut execute));
        assert!(execute.is_null());
        assert!(!proxy_execute_delete(&mut execute));
    }
}

#[link(name = "proxyres_capi_test_log", kind = "static")]
unsafe extern "C" {
    fn proxyres_test_log_cb();
    safe fn proxyres_test_last_message() -> *const c_char;
}

#[test]
fn log_callback() {
    let _lock = log_lock();
    proxy_log_set_error_cb(proxyres_test_log_cb as *const () as *mut c_void);
    log::error!("log callback {}", 42);
    proxy_log_set_error_cb(std::ptr::null_mut());
    let message = borrowed_string(proxyres_test_last_message());
    assert_eq!(message.as_deref(), Some("log callback 42"));
}

#[test]
fn log_levels() {
    let _lock = log_lock();
    let callback = proxyres_test_log_cb as *const () as *mut c_void;
    let last_message = || borrowed_string(proxyres_test_last_message());
    proxy_log_set_warn_cb(callback);
    proxy_log_set_info_cb(callback);
    proxy_log_set_debug_cb(callback);
    assert!(log::log_enabled!(log::Level::Debug));
    log::warn!("warn {}", 1);
    assert_eq!(last_message().as_deref(), Some("warn 1"));
    log::info!("info {}", 2);
    assert_eq!(last_message().as_deref(), Some("info 2"));
    log::debug!("debug {}", 3);
    assert_eq!(last_message().as_deref(), Some("debug 3"));

    proxy_log_set_warn_cb(std::ptr::null_mut());
    proxy_log_set_info_cb(std::ptr::null_mut());
    proxy_log_set_debug_cb(std::ptr::null_mut());
    assert!(!log::log_enabled!(log::Level::Info));
    // Warnings without a callback are printed instead
    log::warn!("printed warning");
    assert_eq!(last_message().as_deref(), Some("debug 3"));
    log::logger().flush();
}
