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
