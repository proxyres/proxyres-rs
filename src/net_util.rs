//! Network address helpers.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

use crate::net_adapter::{DEFAULT_ROUTE_V4, DEFAULT_ROUTE_V6, is_routable, net_adapters, route_local_ip};
use crate::util::strip_ipv6_brackets;

/// Gets the IPv4 address of the local host.
pub(crate) fn my_ip_address() -> Option<String> {
    // Prefer the address of the interface with the default route
    if let Some(ip) = route_local_ip(DEFAULT_ROUTE_V4) {
        return Some(ip.to_string());
    }
    net_adapters()
        .iter()
        .filter(|adapter| adapter.is_connected)
        .flat_map(|adapter| adapter.addresses.iter())
        .find(|ip| ip.is_ipv4() && is_routable(ip))
        .map(IpAddr::to_string)
}

/// Gets the IPv4 and IPv6 addresses of the local host separated by semicolons.
pub(crate) fn my_ip_address_ex() -> Option<String> {
    let mut addresses: Vec<IpAddr> = Vec::new();
    for adapter in net_adapters().iter().filter(|adapter| adapter.is_connected) {
        // List IPv4 addresses before IPv6 addresses for each adapter
        let (ipv4, ipv6): (Vec<IpAddr>, Vec<IpAddr>) = adapter
            .addresses
            .iter()
            .filter(|ip| is_routable(ip))
            .partition(|ip| ip.is_ipv4());
        addresses.extend(ipv4.into_iter().chain(ipv6));
    }
    if addresses.is_empty() {
        addresses.extend(route_local_ip(DEFAULT_ROUTE_V4));
        addresses.extend(route_local_ip(DEFAULT_ROUTE_V6));
    }
    let addresses: Vec<String> = addresses.iter().map(IpAddr::to_string).collect();
    (!addresses.is_empty()).then(|| addresses.join(";"))
}

// Resolve a host name to its addresses
fn resolve(host: &str) -> io::Result<Vec<IpAddr>> {
    let mut addresses: Vec<IpAddr> = Vec::new();
    for address in (strip_ipv6_brackets(host), 0).to_socket_addrs()? {
        if !addresses.contains(&address.ip()) {
            addresses.push(address.ip());
        }
    }
    Ok(addresses)
}

// Error code for a failed resolution
fn error_code(error: &io::Error) -> i32 {
    error.raw_os_error().filter(|&code| code != 0).unwrap_or(-1)
}

/// Resolves a host name to its first IPv4 address.
pub(crate) fn dns_resolve(host: &str) -> Result<String, i32> {
    let addresses = resolve(host).map_err(|error| error_code(&error))?;
    addresses
        .iter()
        .find(|ip| ip.is_ipv4())
        .map(IpAddr::to_string)
        .ok_or(-1)
}

/// Resolves a host name to its IPv4 and IPv6 addresses separated by semicolons.
pub(crate) fn dns_resolve_ex(host: &str) -> Result<String, i32> {
    let addresses = resolve(host).map_err(|error| error_code(&error))?;
    if addresses.is_empty() {
        return Err(-1);
    }
    Ok(addresses.iter().map(IpAddr::to_string).collect::<Vec<_>>().join(";"))
}

// Split cidr notation into its address and prefix length
fn parse_cidr(cidr: &str) -> Option<(&str, u8)> {
    let (ip, prefix) = cidr.split_once('/')?;
    Some((ip, prefix.trim().parse().ok()?))
}

/// Checks if the IPv4 address is in the cidr notation range.
pub(crate) fn is_ipv4_in_cidr_range(ip: &str, cidr: &str) -> bool {
    let Ok(ip) = ip.parse::<Ipv4Addr>() else {
        return false;
    };
    let Some((cidr_ip, prefix)) = parse_cidr(cidr) else {
        return false;
    };
    let Ok(cidr_ip) = cidr_ip.parse::<Ipv4Addr>() else {
        return false;
    };
    if prefix > 32 {
        return false;
    }
    let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
    (u32::from(ip) & mask) == (u32::from(cidr_ip) & mask)
}

/// Checks if the IPv6 address is in the cidr notation range.
pub(crate) fn is_ipv6_in_cidr_range(ip: &str, cidr: &str) -> bool {
    let Ok(ip) = ip.parse::<Ipv6Addr>() else {
        return false;
    };
    let Some((cidr_ip, prefix)) = parse_cidr(cidr) else {
        return false;
    };
    let Ok(cidr_ip) = cidr_ip.parse::<Ipv6Addr>() else {
        return false;
    };
    if prefix > 128 {
        return false;
    }
    let mask = u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0);
    (u128::from(ip) & mask) == (u128::from(cidr_ip) & mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn my_ip_address_single() {
        let address = my_ip_address().expect("no local address");
        assert!(address.contains('.'));
        assert!(!address.contains([';', ':']));
    }

    #[test]
    fn my_ip_address_multiple() {
        let addresses = my_ip_address_ex().expect("no local addresses");
        assert!(addresses.contains(['.', ':']));
    }

    #[test]
    fn dns_resolve_literal() {
        assert_eq!(dns_resolve("127.0.0.1").as_deref(), Ok("127.0.0.1"));
        assert_eq!(dns_resolve_ex("::1").as_deref(), Ok("::1"));
        assert_eq!(dns_resolve_ex("[::1]").as_deref(), Ok("::1"));
        assert!(dns_resolve("::1").is_err());
    }

    #[test]
    fn dns_resolve_localhost() {
        let address = dns_resolve("localhost").expect("unable to resolve localhost");
        assert_eq!(address, "127.0.0.1");
    }

    #[test]
    fn dns_resolve_google() {
        let address = dns_resolve("google.com").expect("unable to resolve google.com");
        assert!(address.contains('.'));
        assert!(!address.contains([';', ':']));
        let addresses = dns_resolve_ex("google.com").expect("unable to resolve google.com");
        assert!(addresses.contains(['.', ':']));
    }

    #[test]
    fn dns_resolve_bad() {
        let error = dns_resolve("hopefully-doesnt-exist.invalid").unwrap_err();
        assert_ne!(error, 0);
    }

    #[test]
    fn ipv4_cidr() {
        assert!(is_ipv4_in_cidr_range("192.167.2.1", "192.168.0.1/8"));
        assert!(is_ipv4_in_cidr_range("10.1.2.3", "0.0.0.0/0"));
        assert!(!is_ipv4_in_cidr_range("192.168.0.0", "192.168.0.1/32"));
        assert!(!is_ipv4_in_cidr_range("192.168.0.1", "192.168.0.1/33"));
        assert!(!is_ipv4_in_cidr_range("192.168.0.1", "192.168.0.1"));
        assert!(!is_ipv4_in_cidr_range("::1", "192.168.0.1/8"));
    }

    #[test]
    fn ipv6_cidr() {
        assert!(is_ipv6_in_cidr_range("::ffff:c0a8:1", "::ffff:c0a0:1/108"));
        assert!(is_ipv6_in_cidr_range("2001:db8::1", "::/0"));
        assert!(!is_ipv6_in_cidr_range("::ffff:c0a8:0", "::ffff:c0a8:1/128"));
        assert!(!is_ipv6_in_cidr_range("::1", "::1/129"));
        assert!(!is_ipv6_in_cidr_range("127.0.0.1", "::1/128"));
    }
}
