//! Proxy configuration from environment variables.

// Only used outside of tests on Linux and other Unix platforms
#![cfg_attr(any(windows, target_os = "macos", target_os = "ios"), allow(dead_code))]

use super::ConfigBackend;

// Get a non-empty environment variable by its lowercase name and then by its uppercase name
// https://unix.stackexchange.com/questions/212894
fn proxy_env_var(name: &str, check_uppercase: bool, var: impl Fn(&str) -> Option<String>) -> Option<String> {
    let non_empty = |name: &str| var(name).filter(|value| !value.is_empty());
    non_empty(name).or_else(|| match check_uppercase {
        true => non_empty(&name.to_ascii_uppercase()),
        false => None,
    })
}

fn proxy(scheme: &str, var: impl Fn(&str) -> Option<String>) -> Option<String> {
    // Don't check HTTP_PROXY due to CGI environment variable creation
    // https://everything.curl.dev/usingcurl/proxies/env
    let check_uppercase = !scheme.eq_ignore_ascii_case("http");
    proxy_env_var(&format!("{}_proxy", scheme.to_ascii_lowercase()), check_uppercase, var)
}

fn bypass_list(var: impl Fn(&str) -> Option<String>) -> Option<String> {
    let no_proxy = proxy_env_var("no_proxy", true, var)?;
    // Remove last separator
    Some(no_proxy.trim_end_matches(',').to_string())
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

pub(super) struct EnvConfig;

impl ConfigBackend for EnvConfig {
    fn auto_discover(&self) -> bool {
        false
    }

    fn auto_config_url(&self) -> Option<String> {
        None
    }

    fn proxy(&self, scheme: &str) -> Option<String> {
        proxy(scheme, env_var)
    }

    fn bypass_list(&self) -> Option<String> {
        bypass_list(env_var)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |name| vars.get(name).cloned()
    }

    #[test]
    fn env_proxy() {
        let var = lookup(&[
            ("http_proxy", "my.proxy3.com:8003"),
            ("HTTPS_PROXY", "my.proxy4.com:8004"),
            ("ftp_proxy", ""),
            ("FTP_PROXY", "my.proxy5.com:8005"),
        ]);
        assert_eq!(proxy("http", &var).as_deref(), Some("my.proxy3.com:8003"));
        assert_eq!(proxy("https", &var).as_deref(), Some("my.proxy4.com:8004"));
        // Empty lowercase variable falls back to uppercase variable
        assert_eq!(proxy("ftp", &var).as_deref(), Some("my.proxy5.com:8005"));
        assert_eq!(proxy("socks", &var), None);
    }

    #[test]
    fn env_http_proxy_uppercase_ignored() {
        let var = lookup(&[("HTTP_PROXY", "my.proxy3.com:8003")]);
        assert_eq!(proxy("http", &var), None);
    }

    #[test]
    fn env_bypass_list() {
        let var = lookup(&[("NO_PROXY", "localhost,127.0.0.0/8,")]);
        assert_eq!(bypass_list(&var).as_deref(), Some("localhost,127.0.0.0/8"));
        assert_eq!(bypass_list(lookup(&[("no_proxy", "")])), None);
    }
}
