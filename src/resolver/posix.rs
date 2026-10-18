//! Built-in proxy resolver using WPAD discovery and PAC script execution.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use super::{Cancel, ResolverBackend};
use crate::fetch::fetch_get;
use crate::threadpool::POOL;
use crate::util::convert_proxy_list_to_uri_list;
use crate::wpad::{wpad_dhcp, wpad_dns};
use crate::{Error, config, execute};

const WPAD_DHCP_TIMEOUT: Duration = Duration::from_secs(3);
const WPAD_EXPIRE: Duration = Duration::from_secs(300);
const SCRIPT_EXPIRE: Duration = Duration::from_secs(300);
const SCRIPT_FAILURE_EXPIRE: Duration = Duration::from_secs(30);

struct CachedScript {
    url: String,
    script: Result<Arc<str>, Error>,
    fetched_at: Instant,
}

/// Cache of the discovered WPAD url and the last downloaded PAC script.
struct WpadCache {
    discovered_url: Option<String>,
    discovered_at: Option<Instant>,
    script: Option<CachedScript>,
}

static CACHE: Mutex<WpadCache> = Mutex::new(WpadCache {
    discovered_url: None,
    discovered_at: None,
    script: None,
});

fn is_fresh(time: Option<Instant>, expire: Duration) -> bool {
    time.is_some_and(|time| time.elapsed() < expire)
}

impl WpadCache {
    // Discover the proxy auto-config url using DHCP and then DNS
    fn discover(&mut self) -> Option<String> {
        if is_fresh(self.discovered_at, WPAD_EXPIRE) {
            return self.discovered_url.clone();
        }

        log::info!("Discovering proxy auto config using WPAD (DHCP)");
        let mut url = wpad_dhcp(WPAD_DHCP_TIMEOUT);
        if url.is_none() {
            log::info!("Discovering proxy auto config using WPAD (DNS)");
            if let Some((dns_url, script)) = wpad_dns(None) {
                // DNS discovery downloads the script so cache it with its url
                self.script = Some(CachedScript {
                    url: dns_url.clone(),
                    script: Ok(script.into()),
                    fetched_at: Instant::now(),
                });
                url = Some(dns_url);
            }
        }

        self.discovered_url = url.clone();
        self.discovered_at = Some(Instant::now());
        url
    }

    // Get the PAC script for a url, downloading it if it is not cached
    fn script(&mut self, url: &str) -> Result<Arc<str>, Error> {
        if let Some(cached) = &self.script {
            let expire = match cached.script {
                Ok(_) => SCRIPT_EXPIRE,
                Err(_) => SCRIPT_FAILURE_EXPIRE,
            };
            if cached.url == url && is_fresh(Some(cached.fetched_at), expire) {
                return cached.script.clone();
            }
        }

        log::info!("Fetching proxy auto config script from {url}");
        let script = fetch_get(url).map(Arc::from);
        if let Err(error) = &script {
            log::error!("Unable to fetch proxy auto config script ({error})");
        }
        self.script = Some(CachedScript {
            url: url.to_string(),
            script: script.clone(),
            fetched_at: Instant::now(),
        });
        script
    }
}

/// Discovers the PAC url with WPAD and returns its script.
pub(super) fn discover_script(cancel: &Cancel) -> Result<Option<Arc<str>>, Error> {
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(auto_config_url) = cache.discover() else {
        return Ok(None);
    };
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    cache.script(&auto_config_url).map(Some)
}

/// Starts discovering the PAC script with WPAD on the thread pool if auto-discovery is enabled.
pub(super) fn prefetch() {
    if !config::auto_discover() {
        return;
    }
    POOL.enqueue(|| {
        // Resolutions started in the meantime wait for the cache lock and reuse the result
        if let Err(error) = discover_script(&Cancel::default()) {
            log::debug!("Unable to prefetch proxy auto config script ({error})");
        }
    });
}

pub(super) struct PosixResolver;

impl PosixResolver {
    /// Discovers or reads the PAC url and returns its script.
    fn script(cancel: &Cancel) -> Result<Option<Arc<str>>, Error> {
        // Discovery and downloading are serialized so concurrent resolutions share the result
        let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
        let mut auto_config_url = None;
        if config::auto_discover() {
            auto_config_url = cache.discover();
        }
        // Use manually specified proxy auto configuration
        let Some(auto_config_url) = auto_config_url.or_else(config::auto_config_url) else {
            return Ok(None);
        };
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        cache.script(&auto_config_url).map(Some)
    }
}

impl ResolverBackend for PosixResolver {
    fn get_proxies_for_url(&self, url: &str, cancel: &Cancel) -> Result<String, Error> {
        // Use direct connection since WPAD didn't result in a proxy auto-config url
        let Some(script) = Self::script(cancel)? else {
            return Ok("direct://".to_string());
        };
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }

        // Convert the result of FindProxyForURL to a uri list using http for type PROXY
        let list = execute::get_proxies_for_url(&script, url)?;
        let uri_list = convert_proxy_list_to_uri_list(&list, Some("http"));
        if uri_list.is_empty() {
            return Ok("direct://".to_string());
        }
        Ok(uri_list)
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;
    use crate::config::{set_auto_config_url_override, set_proxy_override, test_lock};
    use crate::fetch::tests::serve_once;

    const SCRIPT: &str = "function FindProxyForURL(url, host) { return 'DIRECT'; }";

    // Get a url that refuses connections, so only a cached script can be returned for it
    fn refused_url() -> String {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        format!("http://127.0.0.1:{port}/wpad.dat")
    }

    fn served_url() -> String {
        serve_once(format!("HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n{SCRIPT}", SCRIPT.len()).into_bytes())
    }

    fn cached(url: &str, script: Result<Arc<str>, Error>, fetched_at: Instant) -> WpadCache {
        WpadCache {
            discovered_url: None,
            discovered_at: None,
            script: Some(CachedScript {
                url: url.to_string(),
                script,
                fetched_at,
            }),
        }
    }

    #[test]
    fn discover_returns_fresh_result() {
        let mut cache = WpadCache {
            discovered_url: Some("http://wpad.example.com/wpad.dat".to_string()),
            discovered_at: Some(Instant::now()),
            script: None,
        };
        assert_eq!(cache.discover().as_deref(), Some("http://wpad.example.com/wpad.dat"));
    }

    #[test]
    fn script_is_cached() {
        let url = refused_url();
        let mut cache = cached(&url, Ok("cached".into()), Instant::now());
        assert_eq!(cache.script(&url).as_deref(), Ok("cached"));
        // Failures are cached too
        let failure = Error::Fetch("cached".to_string());
        let mut cache = cached(&url, Err(failure.clone()), Instant::now());
        assert_eq!(cache.script(&url).as_deref(), Err(&failure));
    }

    #[test]
    fn script_is_downloaded_for_another_url() {
        let mut cache = cached(&refused_url(), Ok("other".into()), Instant::now());
        let url = served_url();
        assert_eq!(cache.script(&url).as_deref(), Ok(SCRIPT));
    }

    #[test]
    fn failure_expires() {
        // The monotonic clock may not reach back far enough shortly after boot
        let Some(fetched_at) = Instant::now().checked_sub(SCRIPT_FAILURE_EXPIRE + Duration::from_secs(1)) else {
            return;
        };
        let url = served_url();
        let mut cache = cached(&url, Err(Error::Fetch("expired".to_string())), fetched_at);
        assert_eq!(cache.script(&url).as_deref(), Ok(SCRIPT));
    }

    #[test]
    fn discover_script_uses_cache() {
        let _lock = test_lock();
        let url = "http://wpad.example.com/wpad.dat";
        let set_cache = |discovered_url: Option<&str>| {
            *CACHE.lock().unwrap_or_else(PoisonError::into_inner) = WpadCache {
                discovered_url: discovered_url.map(str::to_string),
                discovered_at: Some(Instant::now()),
                ..cached(url, Ok("cached".into()), Instant::now())
            };
        };
        set_cache(Some(url));
        assert_eq!(discover_script(&Cancel::default()), Ok(Some("cached".into())));
        let cancel = Cancel::default();
        cancel.cancel();
        assert_eq!(discover_script(&cancel), Err(Error::Cancelled));
        // Nothing was discovered
        set_cache(None);
        assert_eq!(discover_script(&Cancel::default()), Ok(None));
        *CACHE.lock().unwrap_or_else(PoisonError::into_inner) = WpadCache {
            discovered_url: None,
            discovered_at: None,
            script: None,
        };
    }

    #[test]
    fn cancelled_before_download() {
        let _lock = test_lock();
        set_auto_config_url_override(Some(&refused_url()));
        let cancel = Cancel::default();
        cancel.cancel();
        assert_eq!(
            PosixResolver.get_proxies_for_url("http://example.com/", &cancel),
            Err(Error::Cancelled)
        );
        set_auto_config_url_override(None);
    }

    #[test]
    fn direct_without_script() {
        let _lock = test_lock();
        // A proxy override disables auto-discovery, leaving only a configured PAC url
        set_proxy_override(Some("proxy:8080"));
        if config::auto_config_url().is_none() {
            assert_eq!(
                PosixResolver
                    .get_proxies_for_url("http://example.com/", &Cancel::default())
                    .as_deref(),
                Ok("direct://")
            );
        }
        set_proxy_override(None);
    }
}
