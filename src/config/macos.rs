//! Proxy configuration from CFNetwork on macOS and iOS.

use super::ConfigBackend;
#[cfg(target_os = "macos")]
use crate::macos::cf::cf_dictionary_strings;
use crate::macos::cf::{CfConstString, CfOwned, cf_dictionary_bool, cf_dictionary_i64, cf_dictionary_string};
use crate::macos::cf_network::{
    CFNetworkCopySystemProxySettings, kCFNetworkProxiesHTTPEnable, kCFNetworkProxiesHTTPPort,
    kCFNetworkProxiesHTTPProxy, kCFNetworkProxiesProxyAutoConfigEnable, kCFNetworkProxiesProxyAutoConfigURLString,
};
#[cfg(target_os = "macos")]
use crate::macos::cf_network::{
    kCFNetworkProxiesExceptionsList, kCFNetworkProxiesExcludeSimpleHostnames, kCFNetworkProxiesFTPEnable,
    kCFNetworkProxiesFTPPort, kCFNetworkProxiesFTPProxy, kCFNetworkProxiesHTTPSEnable, kCFNetworkProxiesHTTPSPort,
    kCFNetworkProxiesHTTPSProxy, kCFNetworkProxiesProxyAutoDiscoveryEnable, kCFNetworkProxiesRTSPEnable,
    kCFNetworkProxiesRTSPPort, kCFNetworkProxiesRTSPProxy, kCFNetworkProxiesSOCKSEnable, kCFNetworkProxiesSOCKSPort,
    kCFNetworkProxiesSOCKSProxy,
};

// Copy the system proxy settings
fn system_proxy_settings() -> Option<CfOwned> {
    CfOwned::new(CFNetworkCopySystemProxySettings() as _)
}

// Get the enable, host, and port keys for a scheme's proxy settings
#[cfg(target_os = "macos")]
fn proxy_keys(scheme: &str) -> (&'static CfConstString, &'static CfConstString, &'static CfConstString) {
    if scheme.starts_with("https") {
        (
            &kCFNetworkProxiesHTTPSEnable,
            &kCFNetworkProxiesHTTPSProxy,
            &kCFNetworkProxiesHTTPSPort,
        )
    } else if scheme.starts_with("socks") {
        (
            &kCFNetworkProxiesSOCKSEnable,
            &kCFNetworkProxiesSOCKSProxy,
            &kCFNetworkProxiesSOCKSPort,
        )
    } else if scheme.starts_with("ftp") {
        (
            &kCFNetworkProxiesFTPEnable,
            &kCFNetworkProxiesFTPProxy,
            &kCFNetworkProxiesFTPPort,
        )
    } else if scheme.starts_with("rtsp") {
        (
            &kCFNetworkProxiesRTSPEnable,
            &kCFNetworkProxiesRTSPProxy,
            &kCFNetworkProxiesRTSPPort,
        )
    } else {
        (
            &kCFNetworkProxiesHTTPEnable,
            &kCFNetworkProxiesHTTPProxy,
            &kCFNetworkProxiesHTTPPort,
        )
    }
}

// Get the enable, host, and port keys for a scheme's proxy settings
#[cfg(target_os = "ios")]
fn proxy_keys(_scheme: &str) -> (&'static CfConstString, &'static CfConstString, &'static CfConstString) {
    // iOS only has settings for an HTTP proxy, which is used for all schemes
    (
        &kCFNetworkProxiesHTTPEnable,
        &kCFNetworkProxiesHTTPProxy,
        &kCFNetworkProxiesHTTPPort,
    )
}

pub(super) struct MacConfig;

impl ConfigBackend for MacConfig {
    #[cfg(target_os = "macos")]
    fn auto_discover(&self) -> bool {
        let Some(settings) = system_proxy_settings() else {
            return false;
        };
        cf_dictionary_bool(&settings, &kCFNetworkProxiesProxyAutoDiscoveryEnable)
    }

    // iOS has no setting for proxy auto-discovery
    #[cfg(target_os = "ios")]
    fn auto_discover(&self) -> bool {
        false
    }

    fn auto_config_url(&self) -> Option<String> {
        let settings = system_proxy_settings()?;
        if !cf_dictionary_bool(&settings, &kCFNetworkProxiesProxyAutoConfigEnable) {
            return None;
        }
        cf_dictionary_string(&settings, &kCFNetworkProxiesProxyAutoConfigURLString).filter(|url| !url.is_empty())
    }

    fn proxy(&self, scheme: &str) -> Option<String> {
        let settings = system_proxy_settings()?;
        let (enable, host, port) = proxy_keys(scheme);
        if !cf_dictionary_bool(&settings, enable) {
            return None;
        }
        let host = cf_dictionary_string(&settings, host).filter(|host| !host.is_empty())?;
        match cf_dictionary_i64(&settings, port) {
            Some(port) => Some(format!("{host}:{port}")),
            None => Some(host),
        }
    }

    #[cfg(target_os = "macos")]
    fn bypass_list(&self) -> Option<String> {
        let settings = system_proxy_settings()?;
        let mut bypass_list = Vec::new();
        // Add exclusion for simple hostnames to bypass list
        if cf_dictionary_bool(&settings, &kCFNetworkProxiesExcludeSimpleHostnames) {
            bypass_list.push("<local>".to_string());
        }
        bypass_list.extend(cf_dictionary_strings(&settings, &kCFNetworkProxiesExceptionsList));
        bypass_list.retain(|rule| !rule.is_empty());
        (!bypass_list.is_empty()).then(|| bypass_list.join(","))
    }

    // iOS has no setting for a proxy bypass list
    #[cfg(target_os = "ios")]
    fn bypass_list(&self) -> Option<String> {
        None
    }
}
