use std::ffi::{CStr, CString, c_char, c_void};

use proxyres_capi::*;

const PAC_SCRIPT: &str = include_str!("../../tests/pac.js");

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
}

// Config overrides are global so all config and resolver checks run in one test
#[test]
fn config_and_resolver() {
    let proxy = CString::new("my.proxy:8080").unwrap();
    let bypass_list = CString::new("<local>,*.example.com").unwrap();
    let scheme = CString::new("https").unwrap();
    unsafe {
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
        assert_eq!(proxy_resolver_get_error(std::ptr::null_mut()), -1);
        assert!(!proxy_resolver_wait(std::ptr::null_mut(), 0));
        assert!(!proxy_resolver_delete(std::ptr::null_mut()));
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

        let bad_script = CString::new("function FindProxyForURL(url, host) {").unwrap();
        assert!(!proxy_execute_get_proxies_for_url(
            execute,
            bad_script.as_ptr(),
            url.as_ptr()
        ));
        assert_ne!(proxy_execute_get_error(execute), 0);
        assert!(proxy_execute_get_list(execute).is_null());

        assert!(proxy_execute_delete(&mut execute));
        assert!(execute.is_null());
    }
}

#[link(name = "proxyres_capi_test_log", kind = "static")]
unsafe extern "C" {
    fn proxyres_test_log_cb();
    fn proxyres_test_last_message() -> *const c_char;
}

#[test]
fn log_callback() {
    proxy_log_set_error_cb(proxyres_test_log_cb as *const () as *mut c_void);
    log::error!("log callback {}", 42);
    proxy_log_set_error_cb(std::ptr::null_mut());
    let message = borrowed_string(unsafe { proxyres_test_last_message() });
    assert_eq!(message.as_deref(), Some("log callback 42"));
}
