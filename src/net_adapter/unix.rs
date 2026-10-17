use std::ffi::CStr;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ptr;

use super::NetAdapter;

// Link layer types of ethernet and wireless adapters
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
const LINK_TYPES: [u16; 2] = [1 /* ARPHRD_ETHER */, 6 /* ARPHRD_IEEE802 */];
#[cfg(any(target_os = "macos", target_os = "ios"))]
const LINK_TYPES: [u8; 2] = [0x06 /* IFT_ETHER */, 0x47 /* IFT_IEEE80211 */];

// Convert a socket address to an ip address
unsafe fn sockaddr_to_ip(sockaddr: *const libc::sockaddr) -> Option<IpAddr> {
    if sockaddr.is_null() {
        return None;
    }
    // SAFETY: The caller passes a socket address returned by getifaddrs, and its family
    // determines the layout.
    unsafe {
        match i32::from((*sockaddr).sa_family) {
            libc::AF_INET => {
                let sockaddr = &*(sockaddr as *const libc::sockaddr_in);
                Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(sockaddr.sin_addr.s_addr))))
            }
            libc::AF_INET6 => {
                let sockaddr = &*(sockaddr as *const libc::sockaddr_in6);
                Some(IpAddr::V6(Ipv6Addr::from(sockaddr.sin6_addr.s6_addr)))
            }
            _ => None,
        }
    }
}

// Read the link layer type and hardware address from a link layer socket address
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
unsafe fn sockaddr_to_link(sockaddr: *const libc::sockaddr) -> Option<(u16, Vec<u8>)> {
    // SAFETY: The caller passes a socket address returned by getifaddrs.
    unsafe {
        if sockaddr.is_null() || i32::from((*sockaddr).sa_family) != libc::AF_PACKET {
            return None;
        }
        let link = &*(sockaddr as *const libc::sockaddr_ll);
        let length = usize::from(link.sll_halen).min(link.sll_addr.len());
        Some((link.sll_hatype, link.sll_addr[..length].to_vec()))
    }
}

// Read the link layer type and hardware address from a link layer socket address
#[cfg(any(target_os = "macos", target_os = "ios"))]
unsafe fn sockaddr_to_link(sockaddr: *const libc::sockaddr) -> Option<(u8, Vec<u8>)> {
    // SAFETY: The caller passes a socket address returned by getifaddrs. The hardware address
    // follows the interface name in sdl_data and is bounded by sdl_len.
    unsafe {
        if sockaddr.is_null() || i32::from((*sockaddr).sa_family) != libc::AF_LINK {
            return None;
        }
        let link = &*(sockaddr as *const libc::sockaddr_dl);
        let data_offset = std::mem::offset_of!(libc::sockaddr_dl, sdl_data);
        let start = data_offset + usize::from(link.sdl_nlen);
        let end = start + usize::from(link.sdl_alen);
        if end > usize::from(link.sdl_len) {
            return None;
        }
        let bytes = std::slice::from_raw_parts(sockaddr as *const u8, end);
        Some((link.sdl_type, bytes[start..end].to_vec()))
    }
}

pub(super) fn net_adapters() -> Vec<NetAdapter> {
    let mut ifaddrs: *mut libc::ifaddrs = ptr::null_mut();
    // SAFETY: getifaddrs allocates the list which is freed below.
    if unsafe { libc::getifaddrs(&mut ifaddrs) } != 0 {
        log::error!("Unable to get adapter info ({})", std::io::Error::last_os_error());
        return Vec::new();
    }

    // Each interface has a separate entry for each of its addresses
    let mut adapters: Vec<NetAdapter> = Vec::new();
    let mut link_types: Vec<_> = Vec::new();
    let mut current = ifaddrs;
    // SAFETY: The list returned by getifaddrs remains valid until freeifaddrs.
    unsafe {
        while let Some(ifa) = current.as_ref() {
            current = ifa.ifa_next;

            if ifa.ifa_name.is_null() || ifa.ifa_flags & libc::IFF_LOOPBACK as u32 != 0 {
                continue;
            }
            let name = CStr::from_ptr(ifa.ifa_name).to_string_lossy().into_owned();
            let index = match adapters.iter().position(|adapter| adapter.name == name) {
                Some(index) => index,
                None => {
                    let flags = ifa.ifa_flags;
                    adapters.push(NetAdapter {
                        name,
                        is_connected: flags & libc::IFF_UP as u32 != 0 && flags & libc::IFF_RUNNING as u32 != 0,
                        ..Default::default()
                    });
                    link_types.push(None);
                    adapters.len() - 1
                }
            };
            let adapter = &mut adapters[index];

            if let Some((link_type, mac)) = sockaddr_to_link(ifa.ifa_addr) {
                link_types[index] = Some(link_type);
                adapter.mac = mac;
            } else if let Some(ip) = sockaddr_to_ip(ifa.ifa_addr) {
                if ip.is_ipv4() && adapter.netmask.is_none() {
                    if let Some(IpAddr::V4(netmask)) = sockaddr_to_ip(ifa.ifa_netmask) {
                        adapter.netmask = Some(netmask);
                    }
                }
                adapter.addresses.push(ip);
            }
        }
        libc::freeifaddrs(ifaddrs);
    }

    // Ignore non-physical adapters
    adapters
        .into_iter()
        .zip(link_types)
        .filter(|(_, link_type)| link_type.is_some_and(|link_type| LINK_TYPES.contains(&link_type)))
        .map(|(adapter, _)| adapter)
        .collect()
}
