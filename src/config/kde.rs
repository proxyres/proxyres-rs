//! Proxy configuration from the KDE kioslaverc file.

use std::path::PathBuf;

use super::ConfigBackend;
use crate::util_linux::{DesktopEnv, config_value};

const SECTION: &str = "Proxy Settings";

// Values of the ProxyType setting
const PROXY_TYPE_FIXED: u32 = 1;
const PROXY_TYPE_PAC: u32 = 2;
const PROXY_TYPE_WPAD: u32 = 3;
const PROXY_TYPE_ENV: u32 = 4;

// Get the user's home directory from the password database, falling back to HOME
fn home_dir() -> Option<PathBuf> {
    // SAFETY: getpwuid returns a pointer to static storage that is copied before returning.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() && !(*pw).pw_dir.is_null() {
            let dir = std::ffi::CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned();
            return Some(PathBuf::from(dir));
        }
    }
    std::env::var_os("HOME").map(PathBuf::from)
}

// Get the path to the kioslaverc file for the desktop environment
fn config_path(desktop_env: DesktopEnv) -> Option<PathBuf> {
    match desktop_env {
        DesktopEnv::Kde3 | DesktopEnv::Kde4 => {
            // KDEHOME points at the KDE directory itself rather than the user's home directory
            let kde_home = match std::env::var_os("KDEHOME") {
                Some(kde_home) => PathBuf::from(kde_home),
                None if desktop_env == DesktopEnv::Kde3 => home_dir()?.join(".kde"),
                None => home_dir()?.join(".kde4"),
            };
            Some(kde_home.join("share/config/kioslaverc"))
        }
        _ => {
            let config_home = match std::env::var_os("XDG_CONFIG_HOME") {
                Some(config_home) => PathBuf::from(config_home),
                None => home_dir()?.join(".config"),
            };
            Some(config_home.join("kioslaverc"))
        }
    }
}

// KDE 4.8 and later separate the proxy host and port with a space
fn normalize_proxy(value: &str) -> String {
    match value.rsplit_once(' ') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
            format!("{}:{port}", host.trim_end())
        }
        _ => value.to_string(),
    }
}

pub(super) struct KdeConfig {
    config: String,
}

impl KdeConfig {
    pub(super) fn new(desktop_env: DesktopEnv) -> Option<Self> {
        let config = std::fs::read_to_string(config_path(desktop_env)?).ok()?;
        let kde_config = KdeConfig { config };
        // Use environment variables instead
        if kde_config.proxy_type() == Some(PROXY_TYPE_ENV) {
            return None;
        }
        Some(kde_config)
    }

    fn value(&self, key: &str) -> Option<String> {
        config_value(&self.config, SECTION, key)
    }

    fn proxy_type(&self) -> Option<u32> {
        self.value("ProxyType")?.trim().parse().ok()
    }
}

impl ConfigBackend for KdeConfig {
    fn auto_discover(&self) -> bool {
        self.proxy_type() == Some(PROXY_TYPE_WPAD)
    }

    fn auto_config_url(&self) -> Option<String> {
        if self.proxy_type() != Some(PROXY_TYPE_PAC) {
            return None;
        }
        self.value("Proxy Config Script")
    }

    fn proxy(&self, scheme: &str) -> Option<String> {
        if self.proxy_type() != Some(PROXY_TYPE_FIXED) {
            return None;
        }
        // Scheme may be a url
        let scheme = scheme.split(':').next().unwrap_or(scheme).to_ascii_lowercase();
        self.value(&format!("{scheme}Proxy"))
            .map(|value| normalize_proxy(&value))
    }

    fn bypass_list(&self) -> Option<String> {
        self.value("NoProxyFor")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_proxy_cases() {
        assert_eq!(normalize_proxy("http://127.0.0.1 1080"), "http://127.0.0.1:1080");
        assert_eq!(normalize_proxy("http://127.0.0.1:8000/"), "http://127.0.0.1:8000/");
        assert_eq!(normalize_proxy("proxy.com"), "proxy.com");
    }

    #[test]
    fn fixed_proxy() {
        let config = KdeConfig {
            config: "[Proxy Settings]\nProxyType=1\nhttpProxy=http://my.proxy1.com 8001\nhttpsProxy=my.proxy2.com:8002\nNoProxyFor=localhost,127.0.0.0/8\n".into(),
        };
        assert!(!config.auto_discover());
        assert_eq!(config.auto_config_url(), None);
        assert_eq!(config.proxy("http").as_deref(), Some("http://my.proxy1.com:8001"));
        assert_eq!(config.proxy("HTTPS").as_deref(), Some("my.proxy2.com:8002"));
        assert_eq!(config.bypass_list().as_deref(), Some("localhost,127.0.0.0/8"));
    }

    #[test]
    fn pac_proxy() {
        let config = KdeConfig {
            config: "[Proxy Settings]\nProxyType=2\nProxy Config Script=http://127.0.0.1:8080/pac.js\n".into(),
        };
        assert_eq!(
            config.auto_config_url().as_deref(),
            Some("http://127.0.0.1:8080/pac.js")
        );
        assert_eq!(config.proxy("http"), None);
    }
}
