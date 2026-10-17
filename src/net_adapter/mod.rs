//! Network adapter enumeration.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr};

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
        Ok(())
    }
}

/// Enumerates ethernet and wireless network adapters.
pub(crate) fn net_adapters() -> Vec<NetAdapter> {
    imp::net_adapters()
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
    fn netmask() {
        assert_eq!(prefix_to_netmask(24), Ipv4Addr::new(255, 255, 255, 0));
        assert_eq!(prefix_to_netmask(0), Ipv4Addr::UNSPECIFIED);
        assert_eq!(prefix_to_netmask(32), Ipv4Addr::BROADCAST);
    }
}
