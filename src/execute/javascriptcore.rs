//! PAC script execution using the JavaScriptCore framework, loaded dynamically at run-time.

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::OnceLock;

use libloading::Library;

use super::{PAC_UTILS, natives};
use crate::Error;
use crate::dylib::dylib_api;

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

dylib_api! {
    struct Api {
        fn JSGlobalContextCreate(*mut c_void) -> JSGlobalContextRef;
        fn JSGlobalContextRelease(JSGlobalContextRef);
        fn JSContextGetGlobalObject(JSContextRef) -> JSObjectRef;
        fn JSStringCreateWithUTF8CString(*const c_char) -> JSStringRef;
        fn JSStringRelease(JSStringRef);
        fn JSStringGetMaximumUTF8CStringSize(JSStringRef) -> usize;
        fn JSStringGetUTF8CString(JSStringRef, *mut c_char, usize) -> usize;
        fn JSEvaluateScript(JSContextRef, JSStringRef, JSObjectRef, JSStringRef, i32, *mut JSValueRef) -> JSValueRef;
        fn JSObjectMakeFunctionWithCallback(JSContextRef, JSStringRef, CallAsFunctionCallback) -> JSObjectRef;
        fn JSObjectSetProperty(JSContextRef, JSObjectRef, JSStringRef, JSValueRef, u32, *mut JSValueRef);
        fn JSObjectGetProperty(JSContextRef, JSObjectRef, JSStringRef, *mut JSValueRef) -> JSValueRef;
        fn JSObjectIsFunction(JSContextRef, JSObjectRef) -> bool;
        fn JSObjectCallAsFunction(
            JSContextRef,
            JSObjectRef,
            JSObjectRef,
            usize,
            *const JSValueRef,
            *mut JSValueRef,
        ) -> JSValueRef;
        fn JSValueIsObject(JSContextRef, JSValueRef) -> bool;
        fn JSValueIsString(JSContextRef, JSValueRef) -> bool;
        fn JSValueToObject(JSContextRef, JSValueRef, *mut JSValueRef) -> JSObjectRef;
        fn JSValueToStringCopy(JSContextRef, JSValueRef, *mut JSValueRef) -> JSStringRef;
        fn JSValueMakeString(JSContextRef, JSStringRef) -> JSValueRef;
        fn JSValueMakeNull(JSContextRef) -> JSValueRef;
    }
}

impl Api {
    fn load() -> Option<Self> {
        // SAFETY: The framework is loaded from its system path and the functions are declared with
        // the signatures in the JavaScriptCore headers.
        unsafe { Api::resolve(Library::new(FRAMEWORK_PATH).ok()?) }
    }

    // Create a JavaScript string from a Rust string
    fn string(&self, s: &str) -> Result<JsString<'_>, Error> {
        let s = CString::new(s).map_err(|_| Error::Script("string contains a null character".to_string()))?;
        // SAFETY: The string is null-terminated.
        let ptr = unsafe { (self.JSStringCreateWithUTF8CString)(s.as_ptr()) };
        if ptr.is_null() {
            return Err(Error::Script("unable to create string".to_string()));
        }
        Ok(JsString { api: self, ptr })
    }

    // Convert a value to a Rust string
    unsafe fn value_to_string(&self, context: JSContextRef, value: JSValueRef) -> Option<String> {
        // SAFETY: The caller passes a valid context and value.
        unsafe {
            let string = (self.JSValueToStringCopy)(context, value, std::ptr::null_mut());
            if string.is_null() {
                return None;
            }
            let string = JsString { api: self, ptr: string };
            Some(string.to_rust())
        }
    }
}

fn api() -> Option<&'static Api> {
    // The framework is never unloaded because threads in its address space may still run
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
            let size = (self.api.JSStringGetMaximumUTF8CStringSize)(self.ptr);
            let mut buffer = vec![0u8; size.max(1)];
            (self.api.JSStringGetUTF8CString)(self.ptr, buffer.as_mut_ptr() as *mut c_char, buffer.len());
            CStr::from_ptr(buffer.as_ptr() as *const c_char)
                .to_string_lossy()
                .into_owned()
        }
    }
}

impl Drop for JsString<'_> {
    fn drop(&mut self) {
        // SAFETY: The string is owned by this wrapper.
        unsafe { (self.api.JSStringRelease)(self.ptr) };
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
        unsafe { (self.api.JSGlobalContextRelease)(self.ptr) };
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
            Some(string) => (api.JSValueMakeString)(context, string.ptr),
            None => (api.JSValueMakeNull)(context),
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
        (api.JSValueMakeNull)(context)
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
        (api.JSEvaluateScript)(
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
        let ptr = (api.JSGlobalContextCreate)(std::ptr::null_mut());
        if ptr.is_null() {
            return Err(Error::Script("unable to create context".to_string()));
        }
        let context = Context { api, ptr };
        let global = (api.JSContextGetGlobalObject)(context.ptr);

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
            let function = (api.JSObjectMakeFunctionWithCallback)(context.ptr, name.ptr, callback);
            if function.is_null() {
                return Err(Error::Script(format!("unable to register {}", name.to_rust())));
            }
            (api.JSObjectSetProperty)(context.ptr, global, name.ptr, function, 0, std::ptr::null_mut());
        }

        // Load Mozilla's PAC helper functions and the PAC script
        evaluate(api, context.ptr, PAC_UTILS)?;
        evaluate(api, context.ptr, script)?;

        let name = api.string("FindProxyForURL")?;
        let value = (api.JSObjectGetProperty)(context.ptr, global, name.ptr, std::ptr::null_mut());
        let function = match (api.JSValueIsObject)(context.ptr, value) {
            true => (api.JSValueToObject)(context.ptr, value, std::ptr::null_mut()),
            false => std::ptr::null_mut(),
        };
        if function.is_null() || !(api.JSObjectIsFunction)(context.ptr, function) {
            return Err(Error::Script("FindProxyForURL is not a function".to_string()));
        }

        // Pass the url and host as arguments rather than as script source
        let url = api.string(url)?;
        let host = api.string(host)?;
        let args = [
            (api.JSValueMakeString)(context.ptr, url.ptr),
            (api.JSValueMakeString)(context.ptr, host.ptr),
        ];
        let mut exception: JSValueRef = std::ptr::null();
        let result = (api.JSObjectCallAsFunction)(
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
        if result.is_null() || !(api.JSValueIsString)(context.ptr, result) {
            return Err(Error::Script("FindProxyForURL did not return a string".to_string()));
        }
        api.value_to_string(context.ptr, result)
            .ok_or_else(|| Error::Script("unable to read result".to_string()))
    }
}
