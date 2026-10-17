//! PAC script execution using the JavaScriptCore framework, loaded dynamically at run-time.

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::OnceLock;

use libloading::Library;

use super::{PAC_UTILS, natives};
use crate::Error;

const FRAMEWORK_PATH: &str = "/System/Library/Frameworks/JavaScriptCore.framework/JavaScriptCore";

type JSContextRef = *const c_void;
type JSGlobalContextRef = *mut c_void;
type JSObjectRef = *mut c_void;
type JSValueRef = *const c_void;
type JSStringRef = *mut c_void;

type CallAsFunctionCallback = unsafe extern "C" fn(
    JSContextRef,
    JSObjectRef,
    JSObjectRef,
    usize,
    *const JSValueRef,
    *mut JSValueRef,
) -> JSValueRef;

struct Api {
    // The framework is never unloaded because threads in its address space may still run
    _library: Library,
    global_context_create: unsafe extern "C" fn(*mut c_void) -> JSGlobalContextRef,
    global_context_release: unsafe extern "C" fn(JSGlobalContextRef),
    context_get_global_object: unsafe extern "C" fn(JSContextRef) -> JSObjectRef,
    string_create_with_utf8_cstring: unsafe extern "C" fn(*const c_char) -> JSStringRef,
    string_release: unsafe extern "C" fn(JSStringRef),
    string_get_maximum_utf8_cstring_size: unsafe extern "C" fn(JSStringRef) -> usize,
    string_get_utf8_cstring: unsafe extern "C" fn(JSStringRef, *mut c_char, usize) -> usize,
    evaluate_script:
        unsafe extern "C" fn(JSContextRef, JSStringRef, JSObjectRef, JSStringRef, i32, *mut JSValueRef) -> JSValueRef,
    object_make_function_with_callback:
        unsafe extern "C" fn(JSContextRef, JSStringRef, CallAsFunctionCallback) -> JSObjectRef,
    object_set_property: unsafe extern "C" fn(JSContextRef, JSObjectRef, JSStringRef, JSValueRef, u32, *mut JSValueRef),
    object_get_property: unsafe extern "C" fn(JSContextRef, JSObjectRef, JSStringRef, *mut JSValueRef) -> JSValueRef,
    object_is_function: unsafe extern "C" fn(JSContextRef, JSObjectRef) -> bool,
    object_call_as_function: unsafe extern "C" fn(
        JSContextRef,
        JSObjectRef,
        JSObjectRef,
        usize,
        *const JSValueRef,
        *mut JSValueRef,
    ) -> JSValueRef,
    value_is_object: unsafe extern "C" fn(JSContextRef, JSValueRef) -> bool,
    value_is_string: unsafe extern "C" fn(JSContextRef, JSValueRef) -> bool,
    value_to_object: unsafe extern "C" fn(JSContextRef, JSValueRef, *mut JSValueRef) -> JSObjectRef,
    value_to_string_copy: unsafe extern "C" fn(JSContextRef, JSValueRef, *mut JSValueRef) -> JSStringRef,
    value_make_string: unsafe extern "C" fn(JSContextRef, JSStringRef) -> JSValueRef,
    value_make_null: unsafe extern "C" fn(JSContextRef) -> JSValueRef,
}

impl Api {
    fn load() -> Option<Self> {
        // SAFETY: The framework is loaded from its system path and the symbols are resolved with
        // the signatures declared in the JavaScriptCore headers.
        unsafe {
            let library = Library::new(FRAMEWORK_PATH).ok()?;
            Some(Api {
                global_context_create: *library.get(b"JSGlobalContextCreate\0").ok()?,
                global_context_release: *library.get(b"JSGlobalContextRelease\0").ok()?,
                context_get_global_object: *library.get(b"JSContextGetGlobalObject\0").ok()?,
                string_create_with_utf8_cstring: *library.get(b"JSStringCreateWithUTF8CString\0").ok()?,
                string_release: *library.get(b"JSStringRelease\0").ok()?,
                string_get_maximum_utf8_cstring_size: *library.get(b"JSStringGetMaximumUTF8CStringSize\0").ok()?,
                string_get_utf8_cstring: *library.get(b"JSStringGetUTF8CString\0").ok()?,
                evaluate_script: *library.get(b"JSEvaluateScript\0").ok()?,
                object_make_function_with_callback: *library.get(b"JSObjectMakeFunctionWithCallback\0").ok()?,
                object_set_property: *library.get(b"JSObjectSetProperty\0").ok()?,
                object_get_property: *library.get(b"JSObjectGetProperty\0").ok()?,
                object_is_function: *library.get(b"JSObjectIsFunction\0").ok()?,
                object_call_as_function: *library.get(b"JSObjectCallAsFunction\0").ok()?,
                value_is_object: *library.get(b"JSValueIsObject\0").ok()?,
                value_is_string: *library.get(b"JSValueIsString\0").ok()?,
                value_to_object: *library.get(b"JSValueToObject\0").ok()?,
                value_to_string_copy: *library.get(b"JSValueToStringCopy\0").ok()?,
                value_make_string: *library.get(b"JSValueMakeString\0").ok()?,
                value_make_null: *library.get(b"JSValueMakeNull\0").ok()?,
                _library: library,
            })
        }
    }

    // Create a JavaScript string from a Rust string
    fn string(&self, s: &str) -> Result<JsString<'_>, Error> {
        let s = CString::new(s).map_err(|_| Error::Script("string contains a null character".to_string()))?;
        // SAFETY: The string is null-terminated.
        let ptr = unsafe { (self.string_create_with_utf8_cstring)(s.as_ptr()) };
        if ptr.is_null() {
            return Err(Error::Script("unable to create string".to_string()));
        }
        Ok(JsString { api: self, ptr })
    }

    // Convert a value to a Rust string
    unsafe fn value_to_string(&self, context: JSContextRef, value: JSValueRef) -> Option<String> {
        // SAFETY: The caller passes a valid context and value.
        unsafe {
            let string = (self.value_to_string_copy)(context, value, std::ptr::null_mut());
            if string.is_null() {
                return None;
            }
            let string = JsString { api: self, ptr: string };
            Some(string.to_rust())
        }
    }
}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(Api::load).as_ref()
}

/// Whether the JavaScriptCore framework is available.
pub(super) fn is_available() -> bool {
    api().is_some()
}

// Owned JavaScript string
struct JsString<'a> {
    api: &'a Api,
    ptr: JSStringRef,
}

impl JsString<'_> {
    fn to_rust(&self) -> String {
        // SAFETY: The buffer is sized by JSStringGetMaximumUTF8CStringSize.
        unsafe {
            let size = (self.api.string_get_maximum_utf8_cstring_size)(self.ptr);
            let mut buffer = vec![0u8; size.max(1)];
            (self.api.string_get_utf8_cstring)(self.ptr, buffer.as_mut_ptr() as *mut c_char, buffer.len());
            CStr::from_ptr(buffer.as_ptr() as *const c_char)
                .to_string_lossy()
                .into_owned()
        }
    }
}

impl Drop for JsString<'_> {
    fn drop(&mut self) {
        // SAFETY: The string is owned by this wrapper.
        unsafe { (self.api.string_release)(self.ptr) };
    }
}

// Owned global context
struct Context<'a> {
    api: &'a Api,
    ptr: JSGlobalContextRef,
}

impl Drop for Context<'_> {
    fn drop(&mut self) {
        // SAFETY: The context is owned by this wrapper.
        unsafe { (self.api.global_context_release)(self.ptr) };
    }
}

// Read the first argument passed to a native function as a string
unsafe fn first_arg(api: &Api, context: JSContextRef, argc: usize, argv: *const JSValueRef) -> Option<String> {
    if argc == 0 || argv.is_null() {
        return None;
    }
    // SAFETY: JavaScriptCore passes argc arguments.
    unsafe { api.value_to_string(context, *argv) }
}

// Return a string from a native function, or null if there is none
unsafe fn make_string(api: &Api, context: JSContextRef, s: Option<String>) -> JSValueRef {
    // SAFETY: The context is valid during the native function call.
    unsafe {
        match s.and_then(|s| api.string(&s).ok()) {
            Some(string) => (api.value_make_string)(context, string.ptr),
            None => (api.value_make_null)(context),
        }
    }
}

unsafe extern "C" fn dns_resolve(
    context: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argc: usize,
    argv: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> JSValueRef {
    let Some(api) = api() else {
        return std::ptr::null();
    };
    // SAFETY: The arguments are provided by JavaScriptCore.
    unsafe {
        let address = first_arg(api, context, argc, argv).and_then(|host| natives::dns_resolve(&host));
        make_string(api, context, address)
    }
}

unsafe extern "C" fn dns_resolve_ex(
    context: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argc: usize,
    argv: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> JSValueRef {
    let Some(api) = api() else {
        return std::ptr::null();
    };
    // SAFETY: The arguments are provided by JavaScriptCore.
    unsafe {
        let host = first_arg(api, context, argc, argv).unwrap_or_default();
        make_string(api, context, Some(natives::dns_resolve_ex(&host)))
    }
}

unsafe extern "C" fn my_ip_address(
    context: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    _argc: usize,
    _argv: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> JSValueRef {
    let Some(api) = api() else {
        return std::ptr::null();
    };
    // SAFETY: The context is provided by JavaScriptCore.
    unsafe { make_string(api, context, Some(natives::my_ip_address())) }
}

unsafe extern "C" fn my_ip_address_ex(
    context: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    _argc: usize,
    _argv: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> JSValueRef {
    let Some(api) = api() else {
        return std::ptr::null();
    };
    // SAFETY: The context is provided by JavaScriptCore.
    unsafe { make_string(api, context, Some(natives::my_ip_address_ex())) }
}

unsafe extern "C" fn alert(
    context: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argc: usize,
    argv: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> JSValueRef {
    let Some(api) = api() else {
        return std::ptr::null();
    };
    // SAFETY: The arguments are provided by JavaScriptCore.
    unsafe {
        natives::alert(&first_arg(api, context, argc, argv).unwrap_or_default());
        (api.value_make_null)(context)
    }
}

// Convert an exception value into a script error
unsafe fn script_error(api: &Api, context: JSContextRef, exception: JSValueRef) -> Error {
    // SAFETY: The caller passes a valid context and exception.
    let message = unsafe { api.value_to_string(context, exception) };
    Error::Script(message.unwrap_or_else(|| "exception".to_string()))
}

// Evaluate code in the context
fn evaluate(api: &Api, context: JSContextRef, code: &str) -> Result<(), Error> {
    let code = api.string(code)?;
    let mut exception: JSValueRef = std::ptr::null();
    // SAFETY: The context and script are valid.
    unsafe {
        (api.evaluate_script)(
            context,
            code.ptr,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            1,
            &mut exception,
        );
        if !exception.is_null() {
            return Err(script_error(api, context, exception));
        }
    }
    Ok(())
}

/// Evaluates the PAC script and calls `FindProxyForURL(url, host)`.
pub(super) fn find_proxy_for_url(script: &str, url: &str, host: &str) -> Result<String, Error> {
    let api = api().ok_or(Error::NoScriptEngine)?;

    // SAFETY: Each object is created and released through the loaded framework and the natives
    // match the callback signature.
    unsafe {
        let ptr = (api.global_context_create)(std::ptr::null_mut());
        if ptr.is_null() {
            return Err(Error::Script("unable to create context".to_string()));
        }
        let context = Context { api, ptr };
        let global = (api.context_get_global_object)(context.ptr);

        // Register native functions with the engine
        let functions: [(&str, CallAsFunctionCallback); 5] = [
            ("dnsResolve", dns_resolve),
            ("dnsResolveEx", dns_resolve_ex),
            ("myIpAddress", my_ip_address),
            ("myIpAddressEx", my_ip_address_ex),
            ("alert", alert),
        ];
        for (name, callback) in functions {
            let name = api.string(name)?;
            let function = (api.object_make_function_with_callback)(context.ptr, name.ptr, callback);
            if function.is_null() {
                return Err(Error::Script(format!("unable to register {}", name.to_rust())));
            }
            (api.object_set_property)(context.ptr, global, name.ptr, function, 0, std::ptr::null_mut());
        }

        // Load Mozilla's PAC helper functions and the PAC script
        evaluate(api, context.ptr, PAC_UTILS)?;
        evaluate(api, context.ptr, script)?;

        let name = api.string("FindProxyForURL")?;
        let value = (api.object_get_property)(context.ptr, global, name.ptr, std::ptr::null_mut());
        let function = match (api.value_is_object)(context.ptr, value) {
            true => (api.value_to_object)(context.ptr, value, std::ptr::null_mut()),
            false => std::ptr::null_mut(),
        };
        if function.is_null() || !(api.object_is_function)(context.ptr, function) {
            return Err(Error::Script("FindProxyForURL is not a function".to_string()));
        }

        // Pass the url and host as arguments rather than as script source
        let url = api.string(url)?;
        let host = api.string(host)?;
        let args = [
            (api.value_make_string)(context.ptr, url.ptr),
            (api.value_make_string)(context.ptr, host.ptr),
        ];
        let mut exception: JSValueRef = std::ptr::null();
        let result = (api.object_call_as_function)(
            context.ptr,
            function,
            std::ptr::null_mut(),
            args.len(),
            args.as_ptr(),
            &mut exception,
        );
        if !exception.is_null() {
            return Err(script_error(api, context.ptr, exception));
        }
        if result.is_null() || !(api.value_is_string)(context.ptr, result) {
            return Err(Error::Script("FindProxyForURL did not return a string".to_string()));
        }
        api.value_to_string(context.ptr, result)
            .ok_or_else(|| Error::Script("unable to read result".to_string()))
    }
}
