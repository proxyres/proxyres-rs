//! WPAD url discovery using the macOS DHCP lease information.

use std::ffi::c_void;

use core_foundation_sys::data::{CFDataGetBytePtr, CFDataGetLength, CFDataRef};
use core_foundation_sys::dictionary::CFDictionaryRef;
use core_foundation_sys::string::CFStringRef;

use super::trim_wpad_url;
use crate::cf::CfOwned;

const DHCP_OPT_WPAD: u8 = 252;

#[link(name = "SystemConfiguration", kind = "framework")]
unsafe extern "C" {
    fn SCDynamicStoreCopyDHCPInfo(store: *const c_void, service_id: CFStringRef) -> CFDictionaryRef;
    fn DHCPInfoGetOptionData(info: CFDictionaryRef, code: u8) -> CFDataRef;
}

/// Gets the WPAD option from the DHCP lease of the primary network service.
pub(super) fn wpad_dhcp_primary() -> Option<String> {
    // SAFETY: The copied dictionary is owned by the caller.
    let dhcp_info = CfOwned::new(unsafe { SCDynamicStoreCopyDHCPInfo(std::ptr::null(), std::ptr::null()) } as _)?;
    // SAFETY: The option data is owned by the dictionary and its length bounds the bytes read.
    unsafe {
        let data = DHCPInfoGetOptionData(dhcp_info.as_ptr() as CFDictionaryRef, DHCP_OPT_WPAD);
        if data.is_null() {
            return None;
        }
        let length = usize::try_from(CFDataGetLength(data)).ok()?;
        trim_wpad_url(std::slice::from_raw_parts(CFDataGetBytePtr(data), length))
    }
}
