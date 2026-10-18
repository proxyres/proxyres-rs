//! Network adapter enumeration.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
use windows as imp;

/// Ethernet or wireless network adapter.
#[derive(Debug, Clone, Default)]
pub(crate) struct NetAdapter {
    pub name: String,
    /// Adapter GUID on Windows, empty elsewhere
    #[cfg_attr(not(windows), allow(dead_code))]
    pub guid: String,
    pub description: String,
    pub mac: Vec<u8>,
    pub addresses: Vec<IpAddr>,
    pub netmask: Option<Ipv4Addr>,
    pub gateway: Option<IpAddr>,
    pub dns_servers: Vec<IpAddr>,
    pub dhcp_server: Option<Ipv4Addr>,
    pub is_connected: bool,
    pub is_dhcp_v4: bool,
    /// Whether the adapter carries the default route
    pub is_primary: bool,
}

impl NetAdapter {
    /// Returns the first IPv4 address of the adapter.
    pub fn ipv4(&self) -> Option<Ipv4Addr> {
        self.addresses.iter().find_map(|address| match address {
            IpAddr::V4(ip) => Some(*ip),
            IpAddr::V6(_) => None,
        })
    }
}

impl fmt::Display for NetAdapter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "adapter: {}", self.name)?;
        if !self.description.is_empty() {
            writeln!(f, "  description: {}", self.description)?;
        }
        if !self.mac.is_empty() {
            let mac: Vec<String> = self.mac.iter().map(|byte| format!("{byte:02x}")).collect();
            writeln!(f, "  mac: {}", mac.join(":"))?;
        }
        for address in &self.addresses {
            writeln!(f, "  ip: {address}")?;
        }
        if let Some(netmask) = self.netmask {
            writeln!(f, "  netmask: {netmask}")?;
        }
        if let Some(gateway) = self.gateway {
            writeln!(f, "  gateway: {gateway}")?;
        }
        for dns_server in &self.dns_servers {
            writeln!(f, "  dns: {dns_server}")?;
        }
        if let Some(dhcp_server) = self.dhcp_server {
            writeln!(f, "  dhcp: {dhcp_server}")?;
        }
        if self.is_connected {
            writeln!(f, "  connected")?;
        }
        if self.is_primary {
            writeln!(f, "  primary")?;
        }
        Ok(())
    }
}

/// Remote addresses used to find the local addresses of the default routes.
pub(crate) const DEFAULT_ROUTE_V4: &str = "198.51.100.1:80";
pub(crate) const DEFAULT_ROUTE_V6: &str = "[2001:db8::1]:80";

/// Checks whether an address is usable for reaching other hosts.
pub(crate) fn is_routable(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified(),
        IpAddr::V6(ip) => !ip.is_loopback() && !ip.is_unicast_link_local() && !ip.is_unspecified(),
    }
}

/// Finds the local address used to reach a remote address without sending any packets.
pub(crate) fn route_local_ip(remote: &str) -> Option<IpAddr> {
    let socket = UdpSocket::bind(if remote.starts_with('[') { "[::]:0" } else { "0.0.0.0:0" }).ok()?;
    socket.connect(remote).ok()?;
    Some(socket.local_addr().ok()?.ip()).filter(is_routable)
}

/// Enumerates ethernet and wireless network adapters.
pub(crate) fn net_adapters() -> Vec<NetAdapter> {
    let mut adapters = imp::net_adapters();
    // The primary adapter owns the local address of the default route
    if let Some(primary_ip) = route_local_ip(DEFAULT_ROUTE_V4) {
        for adapter in &mut adapters {
            adapter.is_primary = adapter.addresses.contains(&primary_ip);
        }
    }
    adapters
}

/// Returns the IPv4 netmask for a prefix length.
#[cfg(any(windows, test))]
pub(crate) fn prefix_to_netmask(prefix: u8) -> Ipv4Addr {
    Ipv4Addr::from(u32::MAX.checked_shl(32 - u32::from(prefix.min(32))).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerate() {
        for adapter in net_adapters() {
            print!("{adapter}");
            assert!(!adapter.name.is_empty());
        }
    }

    #[test]
    fn first_ipv4_address() {
        let adapter = NetAdapter {
            addresses: ["fe80::1", "192.168.1.5", "10.0.0.1"]
                .iter()
                .map(|address| address.parse().unwrap())
                .collect(),
            ..Default::default()
        };
        assert_eq!(adapter.ipv4(), Some(Ipv4Addr::new(192, 168, 1, 5)));
        assert_eq!(NetAdapter::default().ipv4(), None);
    }

    #[test]
    fn netmask() {
        assert_eq!(prefix_to_netmask(24), Ipv4Addr::new(255, 255, 255, 0));
        assert_eq!(prefix_to_netmask(0), Ipv4Addr::UNSPECIFIED);
        assert_eq!(prefix_to_netmask(32), Ipv4Addr::BROADCAST);
    }
}
