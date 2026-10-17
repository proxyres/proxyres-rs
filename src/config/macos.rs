//! Proxy configuration from CFNetwork on macOS and iOS.

use core_foundation_sys::dictionary::CFDictionaryRef;
use core_foundation_sys::string::CFStringRef;

use super::ConfigBackend;
#[cfg(target_os = "macos")]
use crate::cf::cf_dictionary_strings;
use crate::cf::{CfOwned, cf_dictionary_bool, cf_dictionary_i64, cf_dictionary_string};

#[link(name = "CFNetwork", kind = "framework")]
unsafe extern "C" {
    fn CFNetworkCopySystemProxySettings() -> CFDictionaryRef;

    static kCFNetworkProxiesHTTPEnable: CFStringRef;
    static kCFNetworkProxiesHTTPProxy: CFStringRef;
    static kCFNetworkProxiesHTTPPort: CFStringRef;
    static kCFNetworkProxiesProxyAutoConfigEnable: CFStringRef;
    static kCFNetworkProxiesProxyAutoConfigURLString: CFStringRef;
}

// Settings that are only available on macOS
#[cfg(target_os = "macos")]
#[link(name = "CFNetwork", kind = "framework")]
unsafe extern "C" {
    static kCFNetworkProxiesHTTPSEnable: CFStringRef;
    static kCFNetworkProxiesHTTPSProxy: CFStringRef;
    static kCFNetworkProxiesHTTPSPort: CFStringRef;
    static kCFNetworkProxiesSOCKSEnable: CFStringRef;
    static kCFNetworkProxiesSOCKSProxy: CFStringRef;
    static kCFNetworkProxiesSOCKSPort: CFStringRef;
    static kCFNetworkProxiesFTPEnable: CFStringRef;
    static kCFNetworkProxiesFTPProxy: CFStringRef;
    static kCFNetworkProxiesFTPPort: CFStringRef;
    static kCFNetworkProxiesRTSPEnable: CFStringRef;
    static kCFNetworkProxiesRTSPProxy: CFStringRef;
    static kCFNetworkProxiesRTSPPort: CFStringRef;
    static kCFNetworkProxiesProxyAutoDiscoveryEnable: CFStringRef;
    static kCFNetworkProxiesExceptionsList: CFStringRef;
    static kCFNetworkProxiesExcludeSimpleHostnames: CFStringRef;
}

// Copy the system proxy settings
fn system_proxy_settings() -> Option<CfOwned> {
    // SAFETY: The copied dictionary is owned by the caller.
    CfOwned::new(unsafe { CFNetworkCopySystemProxySettings() } as _)
}

// Get the enable, host, and port keys for a scheme's proxy settings
#[cfg(target_os = "macos")]
fn proxy_keys(scheme: &str) -> (CFStringRef, CFStringRef, CFStringRef) {
    let scheme = scheme.to_ascii_lowercase();
    // SAFETY: The keys are constant strings exported by CFNetwork.
    unsafe {
        if scheme.starts_with("https") {
            (
                kCFNetworkProxiesHTTPSEnable,
                kCFNetworkProxiesHTTPSProxy,
                kCFNetworkProxiesHTTPSPort,
            )
        } else if scheme.starts_with("socks") {
            (
                kCFNetworkProxiesSOCKSEnable,
                kCFNetworkProxiesSOCKSProxy,
                kCFNetworkProxiesSOCKSPort,
            )
        } else if scheme.starts_with("ftp") {
            (
                kCFNetworkProxiesFTPEnable,
                kCFNetworkProxiesFTPProxy,
                kCFNetworkProxiesFTPPort,
            )
        } else if scheme.starts_with("rtsp") {
            (
                kCFNetworkProxiesRTSPEnable,
                kCFNetworkProxiesRTSPProxy,
                kCFNetworkProxiesRTSPPort,
            )
        } else {
            (
                kCFNetworkProxiesHTTPEnable,
                kCFNetworkProxiesHTTPProxy,
                kCFNetworkProxiesHTTPPort,
            )
        }
    }
}

// Get the enable, host, and port keys for a scheme's proxy settings
#[cfg(target_os = "ios")]
fn proxy_keys(_scheme: &str) -> (CFStringRef, CFStringRef, CFStringRef) {
    // iOS only has settings for an HTTP proxy, which is used for all schemes
    // SAFETY: The keys are constant strings exported by CFNetwork.
    unsafe {
        (
            kCFNetworkProxiesHTTPEnable,
            kCFNetworkProxiesHTTPProxy,
            kCFNetworkProxiesHTTPPort,
        )
    }
}

pub(super) struct MacConfig;

impl ConfigBackend for MacConfig {
    #[cfg(target_os = "macos")]
    fn auto_discover(&self) -> bool {
        let Some(settings) = system_proxy_settings() else {
            return false;
        };
        // SAFETY: The settings dictionary and key are valid.
        unsafe { cf_dictionary_bool(settings.as_ptr() as _, kCFNetworkProxiesProxyAutoDiscoveryEnable) }
    }

    // iOS has no setting for proxy auto-discovery
    #[cfg(target_os = "ios")]
    fn auto_discover(&self) -> bool {
        false
    }

    fn auto_config_url(&self) -> Option<String> {
        let settings = system_proxy_settings()?;
        let settings = settings.as_ptr() as CFDictionaryRef;
        // SAFETY: The settings dictionary and keys are valid.
        unsafe {
            if !cf_dictionary_bool(settings, kCFNetworkProxiesProxyAutoConfigEnable) {
                return None;
            }
            cf_dictionary_string(settings, kCFNetworkProxiesProxyAutoConfigURLString).filter(|url| !url.is_empty())
        }
    }

    fn proxy(&self, scheme: &str) -> Option<String> {
        let settings = system_proxy_settings()?;
        let settings = settings.as_ptr() as CFDictionaryRef;
        let (enable, host, port) = proxy_keys(scheme);
        // SAFETY: The settings dictionary and keys are valid.
        unsafe {
            if !cf_dictionary_bool(settings, enable) {
                return None;
            }
            let host = cf_dictionary_string(settings, host).filter(|host| !host.is_empty())?;
            match cf_dictionary_i64(settings, port) {
                Some(port) => Some(format!("{host}:{port}")),
                None => Some(host),
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn bypass_list(&self) -> Option<String> {
        let settings = system_proxy_settings()?;
        let settings = settings.as_ptr() as CFDictionaryRef;
        let mut bypass_list = Vec::new();
        // SAFETY: The settings dictionary and keys are valid.
        unsafe {
            // Add exclusion for simple hostnames to bypass list
            if cf_dictionary_bool(settings, kCFNetworkProxiesExcludeSimpleHostnames) {
                bypass_list.push("<local>".to_string());
            }
            bypass_list.extend(cf_dictionary_strings(settings, kCFNetworkProxiesExceptionsList));
        }
        bypass_list.retain(|rule| !rule.is_empty());
        (!bypass_list.is_empty()).then(|| bypass_list.join(","))
    }

    // iOS has no setting for a proxy bypass list
    #[cfg(target_os = "ios")]
    fn bypass_list(&self) -> Option<String> {
        None
    }
}
