# proxyres

Cross-platform proxy resolution for Rust, ported from the C library [proxyres](https://github.com/nmoinvaz/proxyres).

The port is a work in progress. The API may change before the first release.

## Features

* Read the user's proxy configuration from the operating system.
* Execute proxy auto-config (PAC) scripts with the bundled [QuickJS](https://github.com/quickjs-ng/quickjs) engine.
* Resolve the proxies for a url asynchronously using `WinHTTP` on Windows and `CFNetwork` on macOS.
* Or use the built-in resolver, which supports:
  * WPAD discovery using DHCP and DNS.
  * Downloading PAC scripts using HTTP, or HTTPS with the `https` feature.
* Handle multiple simultaneous resolution requests with a thread pool.
* Example command line utility, `proxycli`.

## Platform support

|Platform|Configuration|Resolver|
|:-|:-|:-|
|Windows|WinInet|WinHTTP|
|macOS|CFNetwork|CFNetwork, built-in WPAD discovery|
|Linux|GNOME 3 GSettings, KDE kioslaverc, environment variables|Built-in|

## Example

```rust
use proxyres::resolver::Resolver;

let mut resolver = Resolver::new();
resolver.get_proxies_for_url("https://example.com/")?;
resolver.wait(None);
while let Some(proxy) = resolver.next_proxy() {
    // Each proxy is "scheme://host:port", or "direct://" for a direct connection
    println!("{proxy}");
}
```

Reading the configuration and running PAC scripts directly:

```rust
println!("Auto config url: {:?}", proxyres::config::auto_config_url());

let script = r#"function FindProxyForURL(url, host) { return "PROXY proxy:8080"; }"#;
let list = proxyres::execute::get_proxies_for_url(script, "https://example.com/")?;
```

## Feature flags

|Name|Description|Default|
|:-|:-|:-:|
|`quickjs`|Execute PAC scripts with the bundled QuickJS engine.|on|
|`https`|Download PAC scripts using HTTPS with [ureq](https://github.com/algesten/ureq), verified with the operating system's certificate store. Without it, PAC scripts are downloaded using HTTP 1.0.|off|

## Command line

```sh
cargo run --bin proxycli -- config
cargo run --bin proxycli -- resolve https://example.com/
cargo run --bin proxycli -- execute tests/pac.js https://simple.com/
```

## C API

The [capi](capi) crate builds `proxyres_capi` as a static and a shared library implementing the headers of the C library, which are in [capi/include](capi/include). CI compiles the C library's [proxycli.c](capi/tests/proxycli.c) against it and runs the same checks as the Rust command line utility.

```sh
cargo build -p proxyres-capi --release
```

Strings returned as `char *` must be released with `free`, and strings returned as `const char *` are owned by the instance.

## Differences from the C library

* Global initialization and cleanup are not needed.
* PAC scripts run in QuickJS with memory and time limits instead of the system JavaScript engines.
* The `host` passed to `FindProxyForURL` excludes the port.
* `dnsResolve` returns `null` for hosts that cannot be resolved.
* Proxy bypass rules are evaluated independently of each other.
* GNOME 2 GConf and Windows XP are not supported.

## License

MIT, see [LICENSE.md](LICENSE.md). The PAC helper functions in [src/execute/pac_utils.js](src/execute/pac_utils.js) are from Mozilla and licensed under the MPL 2.0.
