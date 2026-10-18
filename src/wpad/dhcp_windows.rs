//! WPAD url discovery using the Windows DHCP client.

use std::ptr;
use std::sync::Once;

use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, NO_ERROR};
use windows_sys::Win32::NetworkManagement::Dhcp::{
    DHCPAPI_PARAMS, DHCPCAPI_PARAMS_ARRAY, DHCPCAPI_REQUEST_SYNCHRONOUS, DhcpCApiInitialize, DhcpRequestParams,
};

use super::trim_wpad_url;
use crate::net_adapter::NetAdapter;
use crate::util_windows::to_wide;

const DHCP_OPT_WPAD: u32 = 252;

/// Requests the WPAD option from the adapter's DHCP lease.
pub(super) fn wpad_dhcp_adapter(adapter: &NetAdapter) -> Option<String> {
    // The DHCP client API must be initialized before it is used
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let mut version = 0;
        // SAFETY: The version pointer is valid.
        unsafe { DhcpCApiInitialize(&mut version) };
    });

    if adapter.guid.is_empty() {
        return None;
    }
    let adapter_name = to_wide(&adapter.guid);
    let mut buffer = vec![0u8; 4096];
    for _ in 0..2 {
        let mut wpad_params = DHCPAPI_PARAMS {
            Flags: 0,
            OptionId: DHCP_OPT_WPAD,
            IsVendor: 0,
            Data: ptr::null_mut(),
            nBytesData: 0,
        };
        let send_params = DHCPCAPI_PARAMS_ARRAY {
            nParams: 0,
            Params: ptr::null_mut(),
        };
        let request_params = DHCPCAPI_PARAMS_ARRAY {
            nParams: 1,
            Params: &mut wpad_params,
        };
        let mut size = buffer.len() as u32;
        // SAFETY: All pointers are valid for the duration of the call and the returned option data
        // points into the buffer.
        let error = unsafe {
            DhcpRequestParams(
                DHCPCAPI_REQUEST_SYNCHRONOUS,
                ptr::null_mut(),
                adapter_name.as_ptr(),
                ptr::null_mut(),
                send_params,
                request_params,
                buffer.as_mut_ptr(),
                &mut size,
                ptr::null(),
            )
        };
        if error == ERROR_MORE_DATA {
            buffer.resize(size as usize, 0);
            continue;
        }
        if error != NO_ERROR || wpad_params.Data.is_null() || wpad_params.nBytesData == 0 {
            log::debug!("Unable to get WPAD from DHCP for {} ({error})", adapter.name);
            return None;
        }
        // SAFETY: DhcpRequestParams set the option data to a range within the buffer.
        let data = unsafe { std::slice::from_raw_parts(wpad_params.Data, wpad_params.nBytesData as usize) };
        return trim_wpad_url(data);
    }
    None
}
