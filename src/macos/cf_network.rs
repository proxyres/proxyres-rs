//! CFNetwork declarations that core-foundation-sys does not provide.

use std::ffi::c_void;

use core_foundation_sys::array::CFArrayRef;
use core_foundation_sys::base::{CFAllocatorRef, CFIndex};
use core_foundation_sys::dictionary::CFDictionaryRef;
use core_foundation_sys::error::CFErrorRef;
use core_foundation_sys::runloop::CFRunLoopSourceRef;
use core_foundation_sys::stream::CFReadStreamRef;
use core_foundation_sys::string::CFStringRef;
use core_foundation_sys::url::CFURLRef;

use super::cf::CfConstString;

#[repr(C)]
pub(crate) struct StreamClientContext {
    pub(crate) version: CFIndex,
    pub(crate) info: *mut c_void,
    pub(crate) retain: Option<extern "C" fn(*const c_void) -> *const c_void>,
    pub(crate) release: Option<extern "C" fn(*const c_void)>,
    pub(crate) copy_description: Option<extern "C" fn(*const c_void) -> CFStringRef>,
}

type AutoConfigurationClientCallback = unsafe extern "C" fn(*mut c_void, CFArrayRef, CFErrorRef);

pub(crate) type CFHTTPMessageRef = *mut c_void;

#[link(name = "CFNetwork", kind = "framework")]
unsafe extern "C" {
    pub(crate) safe fn CFNetworkCopySystemProxySettings() -> CFDictionaryRef;
    pub(crate) fn CFNetworkExecuteProxyAutoConfigurationURL(
        proxy_auto_config_url: CFURLRef,
        target_url: CFURLRef,
        callback: AutoConfigurationClientCallback,
        client_context: *mut StreamClientContext,
    ) -> CFRunLoopSourceRef;
    pub(crate) fn CFNetworkExecuteProxyAutoConfigurationScript(
        proxy_auto_configuration_script: CFStringRef,
        target_url: CFURLRef,
        callback: AutoConfigurationClientCallback,
        client_context: *mut StreamClientContext,
    ) -> CFRunLoopSourceRef;

    pub(crate) safe static kCFNetworkProxiesHTTPEnable: CfConstString;
    pub(crate) safe static kCFNetworkProxiesHTTPProxy: CfConstString;
    pub(crate) safe static kCFNetworkProxiesHTTPPort: CfConstString;
    pub(crate) safe static kCFNetworkProxiesProxyAutoConfigEnable: CfConstString;
    pub(crate) safe static kCFNetworkProxiesProxyAutoConfigURLString: CfConstString;

    pub(crate) safe static kCFProxyTypeKey: CfConstString;
    pub(crate) safe static kCFProxyHostNameKey: CfConstString;
    pub(crate) safe static kCFProxyPortNumberKey: CfConstString;
    pub(crate) safe static kCFProxyTypeNone: CfConstString;
    pub(crate) safe static kCFProxyTypeHTTP: CfConstString;
    pub(crate) safe static kCFProxyTypeHTTPS: CfConstString;
    pub(crate) safe static kCFProxyTypeSOCKS: CfConstString;
    pub(crate) safe static kCFProxyTypeFTP: CfConstString;

    pub(crate) fn CFHTTPMessageCreateRequest(
        alloc: CFAllocatorRef,
        request_method: CFStringRef,
        url: CFURLRef,
        http_version: CFStringRef,
    ) -> CFHTTPMessageRef;
    pub(crate) fn CFHTTPMessageSetHeaderFieldValue(
        message: CFHTTPMessageRef,
        header_field: CFStringRef,
        value: CFStringRef,
    );
    pub(crate) fn CFHTTPMessageCopyHeaderFieldValue(
        message: CFHTTPMessageRef,
        header_field: CFStringRef,
    ) -> CFStringRef;
    pub(crate) fn CFHTTPMessageGetResponseStatusCode(response: CFHTTPMessageRef) -> CFIndex;
    pub(crate) fn CFReadStreamCreateForHTTPRequest(alloc: CFAllocatorRef, request: CFHTTPMessageRef)
    -> CFReadStreamRef;

    pub(crate) safe static kCFHTTPVersion1_1: CfConstString;
    pub(crate) safe static kCFStreamPropertyHTTPResponseHeader: CfConstString;
    pub(crate) safe static kCFStreamPropertyHTTPShouldAutoredirect: CfConstString;
    pub(crate) safe static kCFStreamPropertyHTTPProxy: CfConstString;
}

// Settings that are only available on macOS
#[cfg(target_os = "macos")]
#[link(name = "CFNetwork", kind = "framework")]
unsafe extern "C" {
    pub(crate) safe static kCFNetworkProxiesHTTPSEnable: CfConstString;
    pub(crate) safe static kCFNetworkProxiesHTTPSProxy: CfConstString;
    pub(crate) safe static kCFNetworkProxiesHTTPSPort: CfConstString;
    pub(crate) safe static kCFNetworkProxiesSOCKSEnable: CfConstString;
    pub(crate) safe static kCFNetworkProxiesSOCKSProxy: CfConstString;
    pub(crate) safe static kCFNetworkProxiesSOCKSPort: CfConstString;
    pub(crate) safe static kCFNetworkProxiesFTPEnable: CfConstString;
    pub(crate) safe static kCFNetworkProxiesFTPProxy: CfConstString;
    pub(crate) safe static kCFNetworkProxiesFTPPort: CfConstString;
    pub(crate) safe static kCFNetworkProxiesRTSPEnable: CfConstString;
    pub(crate) safe static kCFNetworkProxiesRTSPProxy: CfConstString;
    pub(crate) safe static kCFNetworkProxiesRTSPPort: CfConstString;
    pub(crate) safe static kCFNetworkProxiesProxyAutoDiscoveryEnable: CfConstString;
    pub(crate) safe static kCFNetworkProxiesExceptionsList: CfConstString;
    pub(crate) safe static kCFNetworkProxiesExcludeSimpleHostnames: CfConstString;
}
