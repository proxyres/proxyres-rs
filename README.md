# proxyres

[![codecov](https://codecov.io/gh/proxyres/proxyres-rs/branch/main/graph/badge.svg)](https://codecov.io/gh/proxyres/proxyres-rs) [![License: MIT](https://img.shields.io/badge/license-MIT-lightgrey.svg)](https://github.com/proxyres/proxyres-rs/blob/main/LICENSE.md)

Cross-platform proxy resolution for Rust, ported from the C library [proxyres](https://github.com/proxyres/proxyres).

## Features

* Read the user's proxy configuration from the operating system.
* Execute proxy auto-config (PAC) scripts with `JavaScriptCore`, `Windows Script Host`, or the bundled [QuickJS](https://github.com/quickjs-ng/quickjs) engine.
* Resolve the proxies for a url asynchronously using `WinHTTP` on Windows and `CFNetwork` on macOS and iOS.
* Or use the built-in resolver, which supports:
  * WPAD discovery using DHCP and DNS, which can start in the background at startup.
  * Downloading PAC scripts using HTTP, or HTTPS with the `https` feature.
* Handle multiple simultaneous resolution requests with a thread pool.
* C API compatible with the C library's headers.
* Example command line utility, `proxycli`.

## Platform support

|Platform|Configuration|Resolver|Script engine|
|:-|:-|:-|:-|
|Windows|WinInet|WinHTTP|Windows Script Host|
|macOS|CFNetwork|CFNetwork, built-in WPAD discovery|JavaScriptCore|
|iOS|CFNetwork, HTTP proxy and PAC url only|CFNetwork|JavaScriptCore|
|Linux|GNOME 3 GSettings, KDE kioslaverc, environment variables|Built-in|JavaScriptCoreGTK|

Script engines are loaded at run-time, and the bundled QuickJS engine is used instead when the `quickjs` feature is enabled.

## Requirements

|Platform|Minimum version|Notes|
|:-|:-|:-|
|Windows|Windows 10 or Windows Server 2016|32-bit and 64-bit, MSVC and GNU toolchains.|
|macOS|macOS 10.12 on Intel, macOS 11 on Apple silicon||
|iOS|iOS 10, iOS 14 in the simulator|Auto-discovery and bypass lists are not available.|
|Linux|glibc 2.17, kernel 3.2|PAC scripts need JavaScriptCoreGTK 4.0, 4.1, or 6.0 from WebKitGTK 2.22 or later, unless the `quickjs` feature is enabled. GNOME settings need GLib 2.32 or later and `gsettings-desktop-schemas`.|

Building requires Rust 1.85 or later, or Rust 1.87 with the `quickjs` feature. The C API crate also needs a C compiler.

CI runs on Windows Server 2025, macOS 26, the iOS simulator, and Ubuntu 22.04, 24.04, and 26.04.

## Example

```rust
use proxyres::resolver::{Backend, Resolver};

// Optionally start WPAD discovery at startup so the first resolution does not wait for it
Backend::System.prefetch();

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
|`quickjs`|Execute PAC scripts with the bundled QuickJS engine instead of the system engine, with memory and time limits. Adds about 1.2 MB and requires Rust 1.87.|off|
|`https`|Download PAC scripts using HTTPS with [ureq](https://github.com/algesten/ureq), verified with the operating system's certificate store. Without it, PAC scripts are downloaded using HTTP 1.0.|off|

## Command line

The [cli](cli) crate builds `proxycli`, which reads the proxy configuration, executes PAC scripts, and resolves proxies for urls. Add `--verbose` to print progress and debug logging, or `--help` for usage.

```sh
cargo run -p proxyres-cli -- config
cargo run -p proxyres-cli -- config proxy https
cargo run -p proxyres-cli -- resolve https://example.com/
cargo run -p proxyres-cli -- execute tests/pac.js https://simple.com/
```

## C API

The [capi](capi) crate builds `proxyres_capi` as a static and a shared library implementing the headers of the C library, which are in [capi/include](capi/include). This includes the configuration, resolver, execute, and log callback functions. CI compiles the C library's [proxycli.c](capi/tests/proxycli.c) against it and runs the same checks as the Rust command line utility.

```sh
cargo build -p proxyres-capi --release
```

Strings returned as `char *` must be released with `free`, and strings returned as `const char *` are owned by the instance. `proxyres_global_init` and `proxy_resolver_global_init` start WPAD discovery like the C library.

## Differences from the C library

* The url and host are passed to `FindProxyForURL` as arguments, never as script source.
* The `host` passed to `FindProxyForURL` excludes the port.
* `dnsResolve` returns `null` for hosts that cannot be resolved.
* WPAD discovery is supported on macOS.
* GNOME 2 GConf, Windows XP through 8.1, and Windows Store apps are not supported.

## License

MIT, see [LICENSE.md](LICENSE.md). The PAC helper functions in [src/execute/pac_utils.js](src/execute/pac_utils.js) are from Mozilla and licensed under the MPL 2.0.
