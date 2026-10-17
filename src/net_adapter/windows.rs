use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses,
    IF_TYPE_ETHERNET_CSMACD, IF_TYPE_IEEE80211, IP_ADAPTER_ADDRESSES_LH, IP_ADAPTER_DHCP_ENABLED,
};
use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR, SOCKADDR_IN, SOCKADDR_IN6};

use super::{NetAdapter, prefix_to_netmask};
use crate::util_win::wide_ptr_to_string;

// Convert a socket address to an ip address
unsafe fn sockaddr_to_ip(sockaddr: *const SOCKADDR) -> Option<IpAddr> {
    if sockaddr.is_null() {
        return None;
    }
    // SAFETY: The caller passes a socket address returned by the operating system, and its
    // family determines the layout.
    unsafe {
        match (*sockaddr).sa_family {
            AF_INET => {
                let sockaddr = &*(sockaddr as *const SOCKADDR_IN);
                Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(sockaddr.sin_addr.S_un.S_addr))))
            }
            AF_INET6 => {
                let sockaddr = &*(sockaddr as *const SOCKADDR_IN6);
                Some(IpAddr::V6(Ipv6Addr::from(sockaddr.sin6_addr.u.Byte)))
            }
            _ => None,
        }
    }
}

pub(super) fn net_adapters() -> Vec<NetAdapter> {
    let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST;
    let mut buffer: Vec<u64> = Vec::new();
    let mut size: u32 = 16 * 1024;

    // Buffer size can change between calls if adapters are added
    let mut error = ERROR_BUFFER_OVERFLOW;
    for _ in 0..3 {
        buffer.resize((size as usize).div_ceil(size_of::<u64>()), 0);
        // SAFETY: The buffer is at least `size` bytes and aligned for IP_ADAPTER_ADDRESSES_LH.
        error = unsafe {
            GetAdaptersAddresses(
                u32::from(AF_UNSPEC),
                flags,
                ptr::null(),
                buffer.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH,
                &mut size,
            )
        };
        if error != ERROR_BUFFER_OVERFLOW {
            break;
        }
    }
    if error != ERROR_SUCCESS {
        log::error!("Unable to get adapter info ({error})");
        return Vec::new();
    }

    let mut adapters = Vec::new();
    let mut current = buffer.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    // SAFETY: GetAdaptersAddresses populated the buffer with a linked list of adapters whose
    // pointers remain valid while the buffer is alive.
    unsafe {
        while let Some(info) = current.as_ref() {
            current = info.Next;

            // Ignore non-physical adapters
            if info.IfType != IF_TYPE_ETHERNET_CSMACD && info.IfType != IF_TYPE_IEEE80211 {
                continue;
            }

            let mut adapter = NetAdapter {
                name: wide_ptr_to_string(info.FriendlyName),
                description: wide_ptr_to_string(info.Description),
                is_connected: info.OperStatus == IfOperStatusUp,
                ..Default::default()
            };
            if !info.AdapterName.is_null() {
                adapter.guid = std::ffi::CStr::from_ptr(info.AdapterName as *const _)
                    .to_string_lossy()
                    .into_owned();
            }
            let mac_length = (info.PhysicalAddressLength as usize).min(info.PhysicalAddress.len());
            adapter.mac = info.PhysicalAddress[..mac_length].to_vec();

            // Populate DHCPv4 server
            if info.Anonymous2.Flags & IP_ADAPTER_DHCP_ENABLED != 0 {
                if let Some(IpAddr::V4(dhcp_server)) = sockaddr_to_ip(info.Dhcpv4Server.lpSockaddr) {
                    adapter.dhcp_server = Some(dhcp_server);
                    adapter.is_dhcp_v4 = true;
                }
            }

            // Populate adapter ip addresses and netmask
            let mut unicast = info.FirstUnicastAddress;
            while let Some(address) = unicast.as_ref() {
                if let Some(ip) = sockaddr_to_ip(address.Address.lpSockaddr) {
                    if ip.is_ipv4() && adapter.netmask.is_none() {
                        adapter.netmask = Some(prefix_to_netmask(address.OnLinkPrefixLength));
                    }
                    adapter.addresses.push(ip);
                }
                unicast = address.Next;
            }

            // Populate adapter dns servers
            let mut dns_server = info.FirstDnsServerAddress;
            while let Some(address) = dns_server.as_ref() {
                adapter.dns_servers.extend(sockaddr_to_ip(address.Address.lpSockaddr));
                dns_server = address.Next;
            }

            // Populate adapter gateway
            if let Some(gateway) = info.FirstGatewayAddress.as_ref() {
                adapter.gateway = sockaddr_to_ip(gateway.Address.lpSockaddr);
            }

            adapters.push(adapter);
        }
    }
    adapters
}
