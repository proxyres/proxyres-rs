//! Network address helpers.

use std::net::{Ipv4Addr, Ipv6Addr};

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
