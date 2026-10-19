//! PAC script execution using JavaScriptCoreGTK, loaded dynamically at run-time.

use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::sync::OnceLock;

use libloading::Library;

use super::{PAC_UTILS, natives};
use crate::Error;
use crate::dylib::dylib_api;

// Versions of JavaScriptCoreGTK in order of preference
const LIBRARY_NAMES: [&str; 3] = [
    "libjavascriptcoregtk-6.0.so.1",
    "libjavascriptcoregtk-4.1.so.0",
    "libjavascriptcoregtk-4.0.so.18",
];

// Fundamental GLib types
const G_TYPE_STRING: usize = 16 << 2;

dylib_api! {
    struct Api {
        fn g_object_unref(*mut c_void);
        fn g_free(*mut c_void);
        fn g_strdup(*const c_char) -> *mut c_char;
        fn jsc_context_new() -> *mut c_void;
        fn jsc_context_evaluate(*mut c_void, *const c_char, isize) -> *mut c_void;
        fn jsc_context_get_exception(*mut c_void) -> *mut c_void;
        fn jsc_context_set_value(*mut c_void, *const c_char, *mut c_void);
        fn jsc_context_get_value(*mut c_void, *const c_char) -> *mut c_void;
        fn jsc_value_new_functionv(
            *mut c_void,
            *const c_char,
            *const c_void,
            *mut c_void,
            *const c_void,
            usize,
            c_uint,
            *const usize,
        ) -> *mut c_void;
        fn jsc_value_new_string(*mut c_void, *const c_char) -> *mut c_void;
        fn jsc_value_is_function(*mut c_void) -> c_int;
        fn jsc_value_is_string(*mut c_void) -> c_int;
        fn jsc_value_function_callv(*mut c_void, c_uint, *const *mut c_void) -> *mut c_void;
        fn jsc_value_to_string(*mut c_void) -> *mut c_char;
        fn jsc_exception_get_message(*mut c_void) -> *const c_char;
    }
}

// Load JavaScriptCoreGTK, preferring a version that is already loaded by the process
fn load_library() -> Option<Library> {
    use libloading::os::unix::{Library as UnixLibrary, RTLD_LAZY, RTLD_LOCAL};

    for flags in [RTLD_LAZY | RTLD_LOCAL | libc::RTLD_NOLOAD, RTLD_LAZY | RTLD_LOCAL] {
        for name in LIBRARY_NAMES {
            // SAFETY: Loading JavaScriptCoreGTK runs no initialization code that depends on us.
            if let Ok(library) = unsafe { UnixLibrary::open(Some(name), flags) } {
                return Some(library.into());
            }
        }
    }
    None
}

impl Api {
    fn load() -> Option<Self> {
        let library = load_library()?;
        // SAFETY: The functions are declared with the signatures documented by JavaScriptCoreGTK
        // and GLib, which is loaded as a dependency.
        unsafe { Api::resolve(library) }
    }
}

fn api() -> Option<&'static Api> {
    // The library is never unloaded because threads in its address space may still run
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(Api::load).as_ref()
}

/// Whether JavaScriptCoreGTK is installed.
pub(super) fn is_available() -> bool {
    api().is_some()
}

// Owned GObject reference
struct Object<'a> {
    api: &'a Api,
    ptr: *mut c_void,
}

impl<'a> Object<'a> {
    fn new(api: &'a Api, ptr: *mut c_void) -> Option<Self> {
        (!ptr.is_null()).then_some(Object { api, ptr })
    }
}

impl Drop for Object<'_> {
    fn drop(&mut self) {
        // SAFETY: The reference is owned by this wrapper.
        unsafe { (self.api.g_object_unref)(self.ptr) };
    }
}

// Read a string argument passed to a native function
unsafe fn arg_string<'a>(s: *const c_char) -> std::borrow::Cow<'a, str> {
    if s.is_null() {
        return "".into();
    }
    // SAFETY: JavaScriptCoreGTK passes null-terminated strings.
    unsafe { CStr::from_ptr(s) }.to_string_lossy()
}

// Return a string from a native function, which takes ownership and releases it with g_free
fn return_string(s: Option<String>) -> *mut c_char {
    let (Some(api), Some(s)) = (api(), s) else {
        return std::ptr::null_mut();
    };
    let Ok(s) = CString::new(s) else {
        return std::ptr::null_mut();
    };
    // SAFETY: The string is null-terminated.
    unsafe { (api.g_strdup)(s.as_ptr()) }
}

unsafe extern "C" fn dns_resolve(host: *const c_char, _user_data: *mut c_void) -> *mut c_char {
    // SAFETY: The argument is a string from JavaScriptCoreGTK.
    return_string(natives::dns_resolve(&unsafe { arg_string(host) }))
}

unsafe extern "C" fn dns_resolve_ex(host: *const c_char, _user_data: *mut c_void) -> *mut c_char {
    // SAFETY: The argument is a string from JavaScriptCoreGTK.
    return_string(Some(natives::dns_resolve_ex(&unsafe { arg_string(host) })))
}

extern "C" fn my_ip_address(_user_data: *mut c_void) -> *mut c_char {
    return_string(Some(natives::my_ip_address()))
}

extern "C" fn my_ip_address_ex(_user_data: *mut c_void) -> *mut c_char {
    return_string(Some(natives::my_ip_address_ex()))
}

unsafe extern "C" fn alert(message: *const c_char, _user_data: *mut c_void) -> *mut c_char {
    // SAFETY: The argument is a string from JavaScriptCoreGTK.
    natives::alert(&unsafe { arg_string(message) });
    std::ptr::null_mut()
}

// Get the message of the pending exception, if any
fn exception(api: &Api, context: &Object) -> Option<Error> {
    // SAFETY: The context is valid and the exception is owned by the context.
    unsafe {
        let exception = (api.jsc_context_get_exception)(context.ptr);
        if exception.is_null() {
            return None;
        }
        let message = (api.jsc_exception_get_message)(exception);
        let message = match message.is_null() {
            true => "exception".to_string(),
            false => CStr::from_ptr(message).to_string_lossy().into_owned(),
        };
        Some(Error::Script(message))
    }
}

// Evaluate code in the context
fn evaluate(api: &Api, context: &Object, code: &str) -> Result<(), Error> {
    // SAFETY: The code is passed with its length so it does not need a null terminator.
    let result =
        unsafe { (api.jsc_context_evaluate)(context.ptr, code.as_ptr() as *const c_char, code.len() as isize) };
    drop(Object::new(api, result));
    exception(api, context).map_or(Ok(()), Err)
}

/// Evaluates the PAC script and calls `FindProxyForURL(url, host)`.
pub(super) fn find_proxy_for_url(script: &str, url: &str, host: &str) -> Result<String, Error> {
    let api = api().ok_or(Error::NoScriptEngine)?;
    let url = CString::new(url).map_err(|_| Error::Script("url contains a null character".to_string()))?;
    let host = CString::new(host).map_err(|_| Error::Script("host contains a null character".to_string()))?;

    // SAFETY: Each object is created and released through the loaded library and the natives
    // match the signatures registered for them.
    unsafe {
        let context = Object::new(api, (api.jsc_context_new)())
            .ok_or_else(|| Error::Script("unable to create context".to_string()))?;

        // Register native functions with the engine
        let string_params = [G_TYPE_STRING];
        let functions: [(&CStr, *const c_void, c_uint); 5] = [
            (c"dnsResolve", dns_resolve as *const c_void, 1),
            (c"dnsResolveEx", dns_resolve_ex as *const c_void, 1),
            (c"myIpAddress", my_ip_address as *const c_void, 0),
            (c"myIpAddressEx", my_ip_address_ex as *const c_void, 0),
            (c"alert", alert as *const c_void, 1),
        ];
        for (name, callback, param_count) in functions {
            let function = (api.jsc_value_new_functionv)(
                context.ptr,
                name.as_ptr(),
                callback,
                std::ptr::null_mut(),
                std::ptr::null(),
                G_TYPE_STRING,
                param_count,
                string_params.as_ptr(),
            );
            let function = Object::new(api, function)
                .ok_or_else(|| Error::Script(format!("unable to register {}", name.to_string_lossy())))?;
            (api.jsc_context_set_value)(context.ptr, name.as_ptr(), function.ptr);
        }

        // Load Mozilla's PAC helper functions and the PAC script
        evaluate(api, &context, PAC_UTILS)?;
        evaluate(api, &context, script)?;

        let function = Object::new(
            api,
            (api.jsc_context_get_value)(context.ptr, c"FindProxyForURL".as_ptr()),
        );
        let function = match function {
            Some(function) if (api.jsc_value_is_function)(function.ptr) != 0 => function,
            _ => return Err(Error::Script("FindProxyForURL is not a function".to_string())),
        };

        // Pass the url and host as arguments rather than as script source
        let url_value = Object::new(api, (api.jsc_value_new_string)(context.ptr, url.as_ptr()));
        let host_value = Object::new(api, (api.jsc_value_new_string)(context.ptr, host.as_ptr()));
        let (Some(url_value), Some(host_value)) = (url_value, host_value) else {
            return Err(Error::Script("unable to create arguments".to_string()));
        };
        let args = [url_value.ptr, host_value.ptr];
        let result = Object::new(api, (api.jsc_value_function_callv)(function.ptr, 2, args.as_ptr()));
        if let Some(error) = exception(api, &context) {
            return Err(error);
        }
        let result = match result {
            Some(result) if (api.jsc_value_is_string)(result.ptr) != 0 => result,
            _ => return Err(Error::Script("FindProxyForURL did not return a string".to_string())),
        };

        let list = (api.jsc_value_to_string)(result.ptr);
        if list.is_null() {
            return Err(Error::Script("unable to read result".to_string()));
        }
        let string = CStr::from_ptr(list).to_string_lossy().into_owned();
        (api.g_free)(list as *mut c_void);
        Ok(string)
    }
}
