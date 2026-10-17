//! Linux desktop environment helpers.

/// Desktop environment of the current session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DesktopEnv {
    Unknown,
    Kde3,
    Kde4,
    Kde5,
    Gnome2,
    Gnome3,
}

// Get the KDE version from the session version
fn kde_version(session_version: Option<String>) -> DesktopEnv {
    match session_version.and_then(|version| version.trim().parse::<u32>().ok()) {
        None => DesktopEnv::Kde3,
        Some(1..=4) => DesktopEnv::Kde4,
        Some(_) => DesktopEnv::Kde5,
    }
}

/// Gets the desktop environment defined by the process' environment variables.
#[cfg(target_os = "linux")]
pub(crate) fn desktop_env() -> DesktopEnv {
    desktop_env_from(
        |name| std::env::var(name).ok(),
        std::path::Path::new("/usr/bin/gsettings").exists(),
    )
}

pub(crate) fn desktop_env_from(var: impl Fn(&str) -> Option<String>, has_gsettings: bool) -> DesktopEnv {
    let gnome = || match has_gsettings {
        true => DesktopEnv::Gnome3,
        false => DesktopEnv::Gnome2,
    };

    // Since 2012
    if let Some(current_desktop) = var("XDG_CURRENT_DESKTOP").map(|s| s.to_ascii_lowercase()) {
        if ["gnome", "unity", "xfce"]
            .iter()
            .any(|name| current_desktop.contains(name))
        {
            return DesktopEnv::Gnome3;
        }
        if current_desktop.contains("kde") && var("KDE_SESSION_VERSION").is_some() {
            return kde_version(var("KDE_SESSION_VERSION"));
        }
    }

    // Since 2010
    if let Some(desktop_session) = var("DESKTOP_SESSION").map(|s| s.to_ascii_lowercase()) {
        if desktop_session.contains("gnome") || desktop_session.contains("mate") {
            return gnome();
        }
        if desktop_session.contains("kde4") || desktop_session.contains("kde-plasma") {
            return DesktopEnv::Kde4;
        }
        if desktop_session.contains("kde") {
            return kde_version(var("KDE_SESSION_VERSION"));
        }
    }

    if var("GNOME_DESKTOP_SESSION_ID").is_some() {
        return gnome();
    }
    if var("KDE_FULL_SESSION").is_some() {
        return kde_version(var("KDE_SESSION_VERSION"));
    }
    DesktopEnv::Unknown
}

/// Reads a value from an ini config file given the section and key.
///
/// Keys that appear before the first section are matched regardless of the section requested.
pub(crate) fn config_value(config: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = true;
    for line in config.lines().map(|line| line.trim_end_matches('\r')) {
        if let Some(name) = line.strip_prefix('[').and_then(|line| line.strip_suffix(']')) {
            in_section = name == section;
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((line_key, value)) = line.split_once('=') {
            if line_key == key && !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const CONFIG: &str = "
[Notification Messages]
WarnOnLeaveSSLMode=false

[Proxy Settings]
AuthMode=0
NoProxyFor=
Proxy Config Script=
ProxyType=0
ReversedException=false
ftpProxy=
httpProxy=http://127.0.0.1:8000/
httpsProxy=http://127.0.0.1:8001/";

    #[test]
    fn config_value_cases() {
        let tests = [
            ("Proxy Settings", "ProxyType", Some("0")),
            ("Proxy Settings", "httpProxy", Some("http://127.0.0.1:8000/")),
            ("Proxy Settings", "httpsProxy", Some("http://127.0.0.1:8001/")),
            // No value set should return None
            ("Proxy Settings", "ftpProxy", None),
            ("Proxy Settings", "AuthMode", Some("0")),
            ("Proxy Settings", "NoProxyFor", None),
            ("Proxy Settings", "Proxy Config Script", None),
            ("Proxy Settings", "ReversedException", Some("false")),
            // No key should return None
            ("Proxy Settings", "Does Not Exist", None),
            ("Notification Messages", "WarnOnLeaveSSLMode", Some("false")),
            // Keys in other sections are not matched
            ("Notification Messages", "ProxyType", None),
        ];
        for (section, key, expected) in tests {
            assert_eq!(
                config_value(CONFIG, section, key).as_deref(),
                expected,
                "section: {section} key: {key}"
            );
        }
    }

    #[test]
    fn config_value_exact_match() {
        // Keys and sections must match exactly, not by prefix
        assert_eq!(
            config_value("[Proxy Settings]\nProxy=1\n", "Proxy Settings", "ProxyType"),
            None
        );
        assert_eq!(
            config_value("[Proxy]\nProxyType=1\n", "Proxy Settings", "ProxyType"),
            None
        );
        assert_eq!(
            config_value("[Proxy Settings]\r\nProxyType=1\r\n", "Proxy Settings", "ProxyType").as_deref(),
            Some("1")
        );
    }

    fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |name| vars.get(name).cloned()
    }

    type Vars<'a> = &'a [(&'a str, &'a str)];

    #[test]
    fn desktop_env_cases() {
        let tests: [(Vars, bool, DesktopEnv); 9] = [
            (&[("XDG_CURRENT_DESKTOP", "ubuntu:GNOME")], true, DesktopEnv::Gnome3),
            (&[("XDG_CURRENT_DESKTOP", "XFCE")], true, DesktopEnv::Gnome3),
            (
                &[("XDG_CURRENT_DESKTOP", "KDE"), ("KDE_SESSION_VERSION", "6")],
                true,
                DesktopEnv::Kde5,
            ),
            (
                &[("XDG_CURRENT_DESKTOP", "KDE"), ("KDE_SESSION_VERSION", "4")],
                true,
                DesktopEnv::Kde4,
            ),
            (&[("DESKTOP_SESSION", "mate")], false, DesktopEnv::Gnome2),
            (&[("DESKTOP_SESSION", "kde-plasma")], true, DesktopEnv::Kde4),
            (
                &[("KDE_FULL_SESSION", "true"), ("KDE_SESSION_VERSION", "5")],
                true,
                DesktopEnv::Kde5,
            ),
            (&[("KDE_FULL_SESSION", "true")], true, DesktopEnv::Kde3),
            (&[], true, DesktopEnv::Unknown),
        ];
        for (vars, has_gsettings, expected) in tests {
            assert_eq!(
                desktop_env_from(lookup(vars), has_gsettings),
                expected,
                "vars: {vars:?}"
            );
        }
    }
}
