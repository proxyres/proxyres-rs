//! Resolve the proxies to use for a url based on the user's proxy configuration.
//!
//! Manually configured proxies and bypass lists are evaluated directly. Otherwise the proxy
//! auto-config (PAC) script, either configured or discovered with WPAD, is evaluated on a background
//! thread by the operating system's resolver or the built-in resolver.
//!
//! Each proxy is returned in the format `scheme://host:port`, and a direct connection is returned
//! as `direct://`. Every proxy in the list should be attempted in order.
//!
//! ```no_run
//! use proxyres::resolver::Resolver;
//!
//! let mut resolver = Resolver::new();
//! resolver.get_proxies_for_url("https://example.com/").unwrap();
//! resolver.wait(None);
//! while let Some(proxy) = resolver.next_proxy() {
//!     println!("Proxy: {proxy}");
//! }
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::Error;
use crate::config;
use crate::threadpool::POOL;
use crate::util::{should_bypass_proxy, url_from_host, url_scheme};

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod macos;
mod posix;
#[cfg(windows)]
mod winhttp;

/// Proxy resolution implementation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Backend {
    /// The operating system's proxy resolver: WinHTTP on Windows, CFNetwork on macOS and iOS, and
    /// the built-in resolver elsewhere.
    #[default]
    System,
    /// The built-in resolver, which discovers PAC scripts with WPAD using DHCP and DNS and
    /// evaluates them with the available script engine.
    Builtin,
}

impl Backend {
    /// Starts discovering the proxy auto-config (PAC) script with WPAD in the background.
    ///
    /// Call this at startup so the first resolution does not wait for discovery, which can take
    /// several seconds. It does nothing when auto-discovery is disabled or when the backend
    /// discovers scripts itself, as WinHTTP does on Windows.
    pub fn prefetch(self) {
        match self {
            Backend::Builtin => posix::prefetch(),
            #[cfg(windows)]
            Backend::System => {}
            // CFNetwork only needs WPAD discovery when no PAC url is configured
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            Backend::System => {
                if config::auto_config_url().is_none() {
                    posix::prefetch();
                }
            }
            #[cfg(not(any(windows, target_os = "macos", target_os = "ios")))]
            Backend::System => posix::prefetch(),
        }
    }
}

/// Cancellation flag shared with a running resolution.
#[derive(Default)]
pub(crate) struct Cancel(AtomicBool);

impl Cancel {
    pub(crate) fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Resolves proxies for a url without the system configuration's manual settings.
pub(crate) trait ResolverBackend: Send + Sync {
    /// Resolves the proxy list for a url, blocking until complete or cancelled.
    fn get_proxies_for_url(&self, url: &str, cancel: &Cancel) -> Result<String, Error>;

    /// Whether a running resolution stops when cancelled.
    fn supports_cancel(&self) -> bool {
        false
    }
}

// Get the shared instance of a backend
fn backend(backend: Backend) -> Result<&'static dyn ResolverBackend, Error> {
    static POSIX: posix::PosixResolver = posix::PosixResolver;
    match backend {
        Backend::Builtin => Ok(&POSIX),
        Backend::System => {
            #[cfg(windows)]
            {
                static WINHTTP: std::sync::OnceLock<Option<winhttp::WinHttpResolver>> = std::sync::OnceLock::new();
                WINHTTP
                    .get_or_init(winhttp::WinHttpResolver::new)
                    .as_ref()
                    .map(|resolver| resolver as &dyn ResolverBackend)
                    .ok_or(Error::NoResolver)
            }
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            {
                static MACOS: macos::MacResolver = macos::MacResolver;
                Ok(&MACOS)
            }
            #[cfg(not(any(windows, target_os = "macos", target_os = "ios")))]
            Ok(&POSIX)
        }
    }
}

/// Resolves the proxy list from manually configured system settings.
///
/// Returns `None` if a PAC script must be evaluated to determine the proxies.
fn get_proxies_for_url_from_system_config(url: &str) -> Option<String> {
    // Skip if auto-config url evaluation is required for proxy resolution
    if config::auto_config_url().is_some() {
        return None;
    }
    // Use scheme associated with the url when determining proxy
    let scheme = url_scheme(url).unwrap_or("http");
    if let Some(proxy) = config::proxy(scheme) {
        let bypass_list = config::bypass_list();
        if should_bypass_proxy(url, bypass_list.as_deref()) {
            log::info!("Bypassing proxy for {url} ({bypass_list:?})");
            return Some("direct://".to_string());
        }
        // Proxies without a scheme are http proxies
        return Some(url_from_host("http", &proxy));
    }
    // Use direct connection since proxy auto-discovery is not necessary
    if !config::auto_discover() {
        return Some("direct://".to_string());
    }
    None
}

// Resolution running on the thread pool
struct Job {
    result: Mutex<Option<Result<String, Error>>>,
    done: Condvar,
    cancel: Cancel,
}

impl Job {
    fn lock(&self) -> MutexGuard<'_, Option<Result<String, Error>>> {
        self.result.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn complete(&self, result: Result<String, Error>) {
        *self.lock() = Some(result);
        self.done.notify_all();
    }

    // Wait for the result, returning None on timeout
    fn wait(&self, timeout: Option<Duration>) -> Option<Result<String, Error>> {
        let deadline = timeout.map(|timeout| Instant::now() + timeout);
        let mut result = self.lock();
        loop {
            if let Some(result) = result.take() {
                return Some(result);
            }
            result = match deadline {
                None => self.done.wait(result).unwrap_or_else(PoisonError::into_inner),
                Some(deadline) => {
                    let remaining = deadline.checked_duration_since(Instant::now())?;
                    self.done
                        .wait_timeout(result, remaining)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
            };
        }
    }
}

enum State {
    Idle,
    Pending(Arc<Job>),
    Done(Result<String, Error>),
}

/// Asynchronous proxy resolver for urls.
pub struct Resolver {
    backend: Backend,
    state: State,
    // Index of the next proxy returned by next_proxy
    next: Option<usize>,
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Resolver {
    /// Creates a resolver that uses the operating system's proxy resolver.
    pub fn new() -> Self {
        Self::with_backend(Backend::System)
    }

    /// Creates a resolver that uses a specific backend.
    pub fn with_backend(backend: Backend) -> Self {
        Resolver {
            backend,
            state: State::Idle,
            next: None,
        }
    }

    /// Starts resolving the proxies for a url.
    ///
    /// Any resolution already in progress is cancelled. Use [`Resolver::wait`] to wait for the
    /// result.
    pub fn get_proxies_for_url(&mut self, url: &str) -> Result<(), Error> {
        self.cancel();
        self.state = State::Idle;
        self.next = None;

        let backend = backend(self.backend)?;

        // Use system proxy configuration if no auto-discovery mechanism is necessary
        if let Some(list) = get_proxies_for_url_from_system_config(url) {
            self.state = State::Done(Ok(list));
            return Ok(());
        }

        let job = Arc::new(Job {
            result: Mutex::new(None),
            done: Condvar::new(),
            cancel: Cancel::default(),
        });
        let worker_job = job.clone();
        let url = url.to_string();
        let enqueued = POOL.enqueue(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                backend.get_proxies_for_url(&url, &worker_job.cancel)
            }));
            let result = match worker_job.cancel.is_cancelled() {
                true => Err(Error::Cancelled),
                false => result.unwrap_or(Err(Error::Os(-1))),
            };
            worker_job.complete(result);
        });
        if !enqueued {
            return Err(Error::NoResolver);
        }
        self.state = State::Pending(job);
        Ok(())
    }

    /// Waits for the resolution to complete, or indefinitely if no timeout is given.
    ///
    /// Returns `true` if the resolution is complete. Completing resets the position returned by
    /// [`Resolver::next_proxy`] to the start of the list.
    pub fn wait(&mut self, timeout: Option<Duration>) -> bool {
        let result = match &self.state {
            State::Idle => return false,
            State::Done(_) => None,
            State::Pending(job) => match job.wait(timeout) {
                Some(result) => Some(result),
                None => return false,
            },
        };
        if let Some(result) = result {
            self.state = State::Done(result);
        }
        self.next = Some(0);
        true
    }

    /// Gets the comma-separated list of proxies once resolution is complete.
    pub fn list(&self) -> Option<&str> {
        match &self.state {
            State::Done(Ok(list)) => Some(list),
            _ => None,
        }
    }

    /// Gets the proxies once resolution is complete.
    pub fn proxies(&self) -> Vec<&str> {
        self.list().map_or_else(Vec::new, |list| list.split(',').collect())
    }

    /// Gets the next proxy in the list, or `None` once all proxies have been returned.
    pub fn next_proxy(&mut self) -> Option<String> {
        let index = self.next?;
        let proxy = self.list()?.split(',').nth(index)?.to_string();
        self.next = Some(index + 1);
        Some(proxy)
    }

    /// Gets the error if resolution failed.
    pub fn error(&self) -> Option<&Error> {
        match &self.state {
            State::Done(Err(error)) => Some(error),
            _ => None,
        }
    }

    /// Cancels the resolution in progress.
    ///
    /// Returns `true` if the resolution is complete or will stop early. Not all backends can stop
    /// a resolution that is already running.
    pub fn cancel(&mut self) -> bool {
        match &self.state {
            State::Idle => false,
            State::Done(_) => true,
            State::Pending(job) => {
                job.cancel.cancel();
                backend(self.backend).is_ok_and(|backend| backend.supports_cancel())
            }
        }
    }
}

impl Drop for Resolver {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Resolves the proxies for a url, blocking until complete.
///
/// ```no_run
/// for proxy in proxyres::resolver::resolve("https://example.com/").unwrap() {
///     println!("Proxy: {proxy}");
/// }
/// ```
pub fn resolve(url: &str) -> Result<Vec<String>, Error> {
    let mut resolver = Resolver::new();
    resolver.get_proxies_for_url(url)?;
    resolver.wait(None);
    match resolver.error() {
        Some(error) => Err(error.clone()),
        None => Ok(resolver.proxies().into_iter().map(str::to_string).collect()),
    }
}

#[cfg(test)]
mod tests;
