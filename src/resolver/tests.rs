use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use super::*;
use crate::config::{set_auto_config_url_override, set_bypass_list_override, set_proxy_override, test_lock};

const PAC_SCRIPT: &str = include_str!("../../tests/pac.js");
const WAIT_TIMEOUT: Option<Duration> = Some(Duration::from_secs(60));

// Serve a PAC script on a local port for every connection and return its url
fn serve_pac(script: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                continue;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            // Read the request headers
            let mut request = Vec::new();
            let mut buffer = [0u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => request.extend_from_slice(&buffer[..count]),
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/x-ns-proxy-autoconfig\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{script}",
                script.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.shutdown(std::net::Shutdown::Write);
            let _ = std::io::copy(&mut stream, &mut std::io::sink());
        }
    });
    format!("http://127.0.0.1:{port}/pac.js")
}

// Reset all configuration overrides
fn reset_overrides() {
    set_auto_config_url_override(None);
    set_proxy_override(None);
    set_bypass_list_override(None);
}

fn resolve_with(backend: Backend, url: &str) -> Result<String, Error> {
    let mut resolver = Resolver::with_backend(backend);
    resolver.get_proxies_for_url(url)?;
    assert!(resolver.wait(WAIT_TIMEOUT), "timed out resolving {url}");
    match resolver.error() {
        Some(error) => Err(error.clone()),
        None => Ok(resolver.list().unwrap().to_string()),
    }
}

#[test]
fn manual_proxy() {
    let _lock = test_lock();
    reset_overrides();
    set_proxy_override(Some("my.proxy:8080"));
    assert_eq!(
        resolve_with(Backend::System, "http://example.com/").as_deref(),
        Ok("http://my.proxy:8080")
    );
    // Localhost is bypassed by default
    assert_eq!(
        resolve_with(Backend::System, "http://localhost/").as_deref(),
        Ok("direct://")
    );
    // Proxy with a scheme keeps its scheme
    set_proxy_override(Some("https://my.proxy/"));
    assert_eq!(
        resolve_with(Backend::Builtin, "http://example.com/").as_deref(),
        Ok("https://my.proxy:443")
    );
    reset_overrides();
}

#[test]
fn manual_proxy_bypass() {
    let _lock = test_lock();
    reset_overrides();
    set_proxy_override(Some("my.proxy:8080"));
    set_bypass_list_override(Some("*.example.com,10.0.0.0/8"));
    assert_eq!(
        resolve_with(Backend::System, "http://www.example.com/").as_deref(),
        Ok("direct://")
    );
    assert_eq!(
        resolve_with(Backend::System, "http://10.1.2.3/").as_deref(),
        Ok("direct://")
    );
    assert_eq!(
        resolve_with(Backend::System, "http://example.org/").as_deref(),
        Ok("http://my.proxy:8080")
    );
    reset_overrides();
}

#[test]
fn builtin_pac_url() {
    let _lock = test_lock();
    reset_overrides();
    set_auto_config_url_override(Some(&serve_pac(PAC_SCRIPT)));
    let tests = [
        ("https://google.com/", "direct://"),
        ("http://simple.com/", "http://no-such-proxy:80"),
        ("https://simple.com/", "http://no-such-proxy:80"),
        (
            "https://multi.com/",
            "https://some-such-proxy:443,https://any-such-proxy:41",
        ),
        (
            "https://multi-legacy.com/",
            "http://some-such-proxy:443,http://any-such-proxy:41",
        ),
        (
            "https://multi-legacy-direct.com/",
            "http://some-such-proxy:443,http://any-such-proxy:41,direct://",
        ),
    ];
    for (url, expected) in tests {
        assert_eq!(
            resolve_with(Backend::Builtin, url).as_deref(),
            Ok(expected),
            "url: {url}"
        );
    }
    reset_overrides();
}

#[test]
fn system_pac_url() {
    let _lock = test_lock();
    reset_overrides();
    set_auto_config_url_override(Some(&serve_pac(PAC_SCRIPT)));
    let tests = [
        ("https://google.com/", "direct://"),
        ("http://simple.com/", "http://no-such-proxy:80"),
        ("https://simple.com/", "http://no-such-proxy:80"),
        (
            "https://multi-legacy.com/",
            "http://some-such-proxy:443,http://any-such-proxy:41",
        ),
    ];
    for (url, expected) in tests {
        assert_eq!(
            resolve_with(Backend::System, url).as_deref(),
            Ok(expected),
            "url: {url}"
        );
    }
    reset_overrides();
}

#[test]
fn next_proxy() {
    let _lock = test_lock();
    reset_overrides();
    set_auto_config_url_override(Some(&serve_pac(PAC_SCRIPT)));
    let mut resolver = Resolver::with_backend(Backend::Builtin);
    resolver
        .get_proxies_for_url("https://multi-legacy-direct.com/")
        .unwrap();
    assert_eq!(resolver.next_proxy(), None);
    assert!(resolver.wait(WAIT_TIMEOUT));
    assert_eq!(resolver.next_proxy().as_deref(), Some("http://some-such-proxy:443"));
    assert_eq!(resolver.next_proxy().as_deref(), Some("http://any-such-proxy:41"));
    assert_eq!(resolver.next_proxy().as_deref(), Some("direct://"));
    assert_eq!(resolver.next_proxy(), None);
    // Waiting again restarts the list
    assert!(resolver.wait(Some(Duration::ZERO)));
    assert_eq!(resolver.next_proxy().as_deref(), Some("http://some-such-proxy:443"));
    assert_eq!(resolver.proxies().len(), 3);
    reset_overrides();
}

#[test]
fn fetch_failure() {
    let _lock = test_lock();
    reset_overrides();
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    set_auto_config_url_override(Some(&format!("http://127.0.0.1:{port}/pac.js")));
    let result = resolve_with(Backend::Builtin, "http://example.com/");
    assert!(matches!(result, Err(Error::Fetch(_))), "{result:?}");
    reset_overrides();
}

#[test]
fn script_failure() {
    let _lock = test_lock();
    reset_overrides();
    set_auto_config_url_override(Some(&serve_pac("function FindProxyForURL(url, host) { throw 'x'; }")));
    let result = resolve_with(Backend::Builtin, "http://example.com/");
    assert!(matches!(result, Err(Error::Script(_))), "{result:?}");
    reset_overrides();
}

#[test]
fn wait_without_request() {
    let mut resolver = Resolver::new();
    assert!(!resolver.wait(Some(Duration::ZERO)));
    assert!(!resolver.cancel());
    assert_eq!(resolver.list(), None);
    assert_eq!(resolver.error(), None);
}

#[test]
fn resolve_blocking() {
    let _lock = test_lock();
    reset_overrides();
    set_proxy_override(Some("my.proxy:8080"));
    assert_eq!(
        resolve("http://example.com/"),
        Ok(vec!["http://my.proxy:8080".to_string()])
    );
    reset_overrides();
}
