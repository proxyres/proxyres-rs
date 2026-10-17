//! Proxy configuration from WinInet.
//!
//! WinInet is used instead of WinHttpGetIEProxyConfigForCurrentUser because the latter has a
//! network dependency that causes it to be slow <https://stackoverflow.com/questions/2151462/>.

use std::ptr;

use windows_sys::Win32::Foundation::GlobalFree;
use windows_sys::Win32::Networking::WinInet::{
    INTERNET_OPTION_PER_CONNECTION_OPTION, INTERNET_PER_CONN_AUTOCONFIG_URL, INTERNET_PER_CONN_FLAGS,
    INTERNET_PER_CONN_FLAGS_UI, INTERNET_PER_CONN_OPTION_LISTW, INTERNET_PER_CONN_OPTIONW,
    INTERNET_PER_CONN_PROXY_BYPASS, INTERNET_PER_CONN_PROXY_SERVER, InternetQueryOptionW, PROXY_TYPE_AUTO_DETECT,
    PROXY_TYPE_AUTO_PROXY_URL, PROXY_TYPE_PROXY,
};

use super::ConfigBackend;
use crate::util::winhttp_proxy_by_scheme;
use crate::util_win::wide_ptr_to_string;

// Query per-connection options for the default connection
fn query_options(options: &mut [INTERNET_PER_CONN_OPTIONW]) -> bool {
    let mut option_list = INTERNET_PER_CONN_OPTION_LISTW {
        dwSize: size_of::<INTERNET_PER_CONN_OPTION_LISTW>() as u32,
        pszConnection: ptr::null_mut(),
        dwOptionCount: options.len() as u32,
        dwOptionError: 0,
        pOptions: options.as_mut_ptr(),
    };
    let mut option_list_size = size_of::<INTERNET_PER_CONN_OPTION_LISTW>() as u32;
    // SAFETY: The option list points to valid options for the duration of the call.
    unsafe {
        InternetQueryOptionW(
            ptr::null(),
            INTERNET_OPTION_PER_CONNECTION_OPTION,
            &mut option_list as *mut _ as *mut _,
            &mut option_list_size,
        ) != 0
    }
}

// Query the connection flags along with a string option, returning the string if a flag is set
fn query_string_option(option: u32, flag: u32) -> Option<String> {
    let mut options = [
        INTERNET_PER_CONN_OPTIONW::default(),
        INTERNET_PER_CONN_OPTIONW::default(),
    ];
    options[0].dwOption = INTERNET_PER_CONN_FLAGS;
    options[1].dwOption = option;
    if !query_options(&mut options) {
        return None;
    }
    // SAFETY: WinInet populated the union members for the requested options and allocated the
    // string with GlobalAlloc.
    unsafe {
        let value = options[1].Value.pszValue;
        if value.is_null() {
            return None;
        }
        let string = wide_ptr_to_string(value);
        GlobalFree(value as *mut _);
        (options[0].Value.dwValue & flag != 0 && !string.is_empty()).then_some(string)
    }
}

pub(super) struct WindowsConfig;

impl ConfigBackend for WindowsConfig {
    fn auto_discover(&self) -> bool {
        // INTERNET_PER_CONN_FLAGS hides the auto-detection setting if it believes the network does
        // not have WPAD. To get the actual value query INTERNET_PER_CONN_FLAGS_UI first.
        // https://stackoverflow.com/questions/15565997
        for option in [INTERNET_PER_CONN_FLAGS_UI, INTERNET_PER_CONN_FLAGS] {
            let mut options = [INTERNET_PER_CONN_OPTIONW::default()];
            options[0].dwOption = option;
            if query_options(&mut options) {
                // SAFETY: WinInet populated the flags value.
                return unsafe { options[0].Value.dwValue } & PROXY_TYPE_AUTO_DETECT != 0;
            }
        }
        false
    }

    fn auto_config_url(&self) -> Option<String> {
        query_string_option(INTERNET_PER_CONN_AUTOCONFIG_URL, PROXY_TYPE_AUTO_PROXY_URL)
    }

    fn proxy(&self, scheme: &str) -> Option<String> {
        // Proxy may be returned as a list of proxies for different schemes
        let proxy_list = query_string_option(INTERNET_PER_CONN_PROXY_SERVER, PROXY_TYPE_PROXY)?;
        winhttp_proxy_by_scheme(scheme, &proxy_list)
    }

    fn bypass_list(&self) -> Option<String> {
        // Normalize separators for all platforms to comma
        query_string_option(INTERNET_PER_CONN_PROXY_BYPASS, PROXY_TYPE_PROXY).map(|list| list.replace(';', ","))
    }
}
