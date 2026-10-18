//! CFNetwork declarations that core-foundation-sys does not provide.

use std::ffi::c_void;

use core_foundation_sys::array::CFArrayRef;
use core_foundation_sys::base::CFIndex;
use core_foundation_sys::dictionary::CFDictionaryRef;
use core_foundation_sys::error::CFErrorRef;
use core_foundation_sys::runloop::CFRunLoopSourceRef;
use core_foundation_sys::string::CFStringRef;
use core_foundation_sys::url::CFURLRef;

#[repr(C)]
pub(crate) struct StreamClientContext {
    pub(crate) version: CFIndex,
    pub(crate) info: *mut c_void,
    pub(crate) retain: Option<extern "C" fn(*const c_void) -> *const c_void>,
    pub(crate) release: Option<extern "C" fn(*const c_void)>,
    pub(crate) copy_description: Option<extern "C" fn(*const c_void) -> CFStringRef>,
}

type AutoConfigurationClientCallback = unsafe extern "C" fn(*mut c_void, CFArrayRef, CFErrorRef);

#[link(name = "CFNetwork", kind = "framework")]
unsafe extern "C" {
    pub(crate) fn CFNetworkCopySystemProxySettings() -> CFDictionaryRef;
    pub(crate) fn CFNetworkExecuteProxyAutoConfigurationURL(
        proxy_auto_config_url: CFURLRef,
        target_url: CFURLRef,
        callback: AutoConfigurationClientCallback,
        client_context: *mut StreamClientContext,
    ) -> CFRunLoopSourceRef;
    pub(crate) fn CFNetworkCopyProxiesForAutoConfigurationScript(
        proxy_auto_configuration_script: CFStringRef,
        target_url: CFURLRef,
        error: *mut CFErrorRef,
    ) -> CFArrayRef;

    pub(crate) static kCFNetworkProxiesHTTPEnable: CFStringRef;
    pub(crate) static kCFNetworkProxiesHTTPProxy: CFStringRef;
    pub(crate) static kCFNetworkProxiesHTTPPort: CFStringRef;
    pub(crate) static kCFNetworkProxiesProxyAutoConfigEnable: CFStringRef;
    pub(crate) static kCFNetworkProxiesProxyAutoConfigURLString: CFStringRef;

    pub(crate) static kCFProxyTypeKey: CFStringRef;
    pub(crate) static kCFProxyHostNameKey: CFStringRef;
    pub(crate) static kCFProxyPortNumberKey: CFStringRef;
    pub(crate) static kCFProxyTypeNone: CFStringRef;
    pub(crate) static kCFProxyTypeHTTP: CFStringRef;
    pub(crate) static kCFProxyTypeHTTPS: CFStringRef;
    pub(crate) static kCFProxyTypeSOCKS: CFStringRef;
    pub(crate) static kCFProxyTypeFTP: CFStringRef;
}

// Settings that are only available on macOS
#[cfg(target_os = "macos")]
#[link(name = "CFNetwork", kind = "framework")]
unsafe extern "C" {
    pub(crate) static kCFNetworkProxiesHTTPSEnable: CFStringRef;
    pub(crate) static kCFNetworkProxiesHTTPSProxy: CFStringRef;
    pub(crate) static kCFNetworkProxiesHTTPSPort: CFStringRef;
    pub(crate) static kCFNetworkProxiesSOCKSEnable: CFStringRef;
    pub(crate) static kCFNetworkProxiesSOCKSProxy: CFStringRef;
    pub(crate) static kCFNetworkProxiesSOCKSPort: CFStringRef;
    pub(crate) static kCFNetworkProxiesFTPEnable: CFStringRef;
    pub(crate) static kCFNetworkProxiesFTPProxy: CFStringRef;
    pub(crate) static kCFNetworkProxiesFTPPort: CFStringRef;
    pub(crate) static kCFNetworkProxiesRTSPEnable: CFStringRef;
    pub(crate) static kCFNetworkProxiesRTSPProxy: CFStringRef;
    pub(crate) static kCFNetworkProxiesRTSPPort: CFStringRef;
    pub(crate) static kCFNetworkProxiesProxyAutoDiscoveryEnable: CFStringRef;
    pub(crate) static kCFNetworkProxiesExceptionsList: CFStringRef;
    pub(crate) static kCFNetworkProxiesExcludeSimpleHostnames: CFStringRef;
}
