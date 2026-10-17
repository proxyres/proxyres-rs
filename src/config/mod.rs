//! Read the user's proxy configuration.
//!
//! Values are read from the operating system on each call unless an override has been set.
//!
//! ```no_run
//! println!("Auto discover: {}", proxyres::config::auto_discover());
//! println!("Auto config url: {:?}", proxyres::config::auto_config_url());
//! println!("HTTP proxy: {:?}", proxyres::config::proxy("http"));
//! println!("Bypass list: {:?}", proxyres::config::bypass_list());
//! ```

use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

#[cfg(any(all(unix, not(any(target_os = "macos", target_os = "ios"))), test))]
mod env;
#[cfg(target_os = "linux")]
mod gnome3;
#[cfg(target_os = "linux")]
mod kde;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod macos;
#[cfg(windows)]
mod windows;

/// Source of the user's proxy configuration.
pub(crate) trait ConfigBackend: Send + Sync {
    /// Whether proxy auto-discovery (WPAD) is enabled.
    fn auto_discover(&self) -> bool;
    /// Proxy auto-config (PAC) url.
    fn auto_config_url(&self) -> Option<String>;
    /// Proxy for a url scheme in the format `host:port`.
    fn proxy(&self, scheme: &str) -> Option<String>;
    /// Comma-separated proxy bypass list.
    fn bypass_list(&self) -> Option<String>;
}

struct Overrides {
    auto_config_url: Option<String>,
    proxy: Option<String>,
    bypass_list: Option<String>,
}

static OVERRIDES: Mutex<Overrides> = Mutex::new(Overrides {
    auto_config_url: None,
    proxy: None,
    bypass_list: None,
});

static BACKEND: OnceLock<Option<Box<dyn ConfigBackend>>> = OnceLock::new();

fn overrides() -> MutexGuard<'static, Overrides> {
    OVERRIDES.lock().unwrap_or_else(PoisonError::into_inner)
}

fn backend() -> Option<&'static dyn ConfigBackend> {
    BACKEND.get_or_init(create_backend).as_deref()
}

#[cfg(windows)]
fn create_backend() -> Option<Box<dyn ConfigBackend>> {
    Some(Box::new(windows::WindowsConfig))
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn create_backend() -> Option<Box<dyn ConfigBackend>> {
    Some(Box::new(macos::MacConfig))
}

#[cfg(target_os = "linux")]
fn create_backend() -> Option<Box<dyn ConfigBackend>> {
    use crate::util_linux::{DesktopEnv, desktop_env};

    let desktop_env = desktop_env();
    let backend: Option<Box<dyn ConfigBackend>> = match desktop_env {
        DesktopEnv::Gnome3 => gnome3::Gnome3Config::new().map(|config| Box::new(config) as _),
        DesktopEnv::Kde3 | DesktopEnv::Kde4 | DesktopEnv::Kde5 => {
            kde::KdeConfig::new(desktop_env).map(|config| Box::new(config) as _)
        }
        _ => None,
    };
    backend.or_else(|| Some(Box::new(env::EnvConfig)))
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos", target_os = "ios"))))]
fn create_backend() -> Option<Box<dyn ConfigBackend>> {
    Some(Box::new(env::EnvConfig))
}

/// Reads whether proxy auto-discovery (WPAD) is enabled on the user's system.
///
/// Auto-discovery is reported as disabled while an auto config url or proxy override is set.
pub fn auto_discover() -> bool {
    {
        let overrides = overrides();
        if overrides.auto_config_url.is_some() || overrides.proxy.is_some() {
            return false;
        }
    }
    backend().is_some_and(|backend| backend.auto_discover())
}

/// Reads the proxy auto-config (PAC) url configured on the user's system.
pub fn auto_config_url() -> Option<String> {
    if let Some(auto_config_url) = &overrides().auto_config_url {
        return Some(auto_config_url.clone());
    }
    backend()?.auto_config_url()
}

/// Reads the proxy configured on the user's system for a url scheme.
///
/// Values are in the format `host:port` and do not include the scheme.
pub fn proxy(scheme: &str) -> Option<String> {
    if let Some(proxy) = &overrides().proxy {
        return Some(proxy.clone());
    }
    backend()?.proxy(scheme)
}

/// Reads the comma-separated proxy bypass list configured on the user's system.
pub fn bypass_list() -> Option<String> {
    if let Some(bypass_list) = &overrides().bypass_list {
        return Some(bypass_list.clone());
    }
    backend()?.bypass_list()
}

/// Overrides the user's configured proxy auto-config (PAC) url.
pub fn set_auto_config_url_override(auto_config_url: Option<&str>) {
    overrides().auto_config_url = auto_config_url.map(str::to_string);
}

/// Overrides the user's configured proxy for all schemes.
pub fn set_proxy_override(proxy: Option<&str>) {
    overrides().proxy = proxy.map(str::to_string);
}

/// Overrides the user's configured proxy bypass list.
pub fn set_bypass_list_override(bypass_list: Option<&str>) {
    overrides().bypass_list = bypass_list.map(str::to_string);
}

/// Serializes tests that change global configuration overrides.
#[cfg(test)]
pub(crate) fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_auto_config_url() {
        let _lock = test_lock();
        set_auto_config_url_override(Some("http://127.0.0.1:8000/wpad.dat"));
        assert_eq!(auto_config_url().as_deref(), Some("http://127.0.0.1:8000/wpad.dat"));
        assert!(!auto_discover());
        set_auto_config_url_override(None);
    }

    #[test]
    fn override_proxy() {
        let _lock = test_lock();
        set_proxy_override(Some("http://127.0.0.1:8000/"));
        assert_eq!(proxy("http").as_deref(), Some("http://127.0.0.1:8000/"));
        assert_eq!(proxy("https").as_deref(), Some("http://127.0.0.1:8000/"));
        assert!(!auto_discover());
        set_proxy_override(None);
    }

    #[test]
    fn override_both_disables_auto_discover() {
        let _lock = test_lock();
        set_auto_config_url_override(Some("http://127.0.0.1:8000/wpad.dat"));
        set_proxy_override(Some("127.0.0.1:8000"));
        // Clearing one override keeps auto-discovery disabled while the other is set
        set_proxy_override(None);
        assert!(!auto_discover());
        set_auto_config_url_override(None);
    }

    #[test]
    fn override_bypass_list() {
        let _lock = test_lock();
        set_bypass_list_override(Some("<local>"));
        assert_eq!(bypass_list().as_deref(), Some("<local>"));
        set_bypass_list_override(Some(""));
        assert_eq!(bypass_list().as_deref(), Some(""));
        set_bypass_list_override(None);
    }

    #[test]
    fn read_system() {
        println!("auto discover: {}", auto_discover());
        println!("auto config url: {:?}", auto_config_url());
        println!("http proxy: {:?}", proxy("http"));
        println!("https proxy: {:?}", proxy("https"));
        println!("bypass list: {:?}", bypass_list());
    }
}
