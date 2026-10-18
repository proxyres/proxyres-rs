//! Local host name for WPAD discovery using DNS on Windows.

use windows_sys::Win32::System::SystemInformation::{ComputerNameDnsFullyQualified, GetComputerNameExW};

/// Gets the fully qualified domain name of the local host.
pub(super) fn local_fqdn() -> Option<String> {
    let mut size: u32 = 0;
    // SAFETY: The first call only queries the required buffer size.
    unsafe { GetComputerNameExW(ComputerNameDnsFullyQualified, std::ptr::null_mut(), &mut size) };
    let mut buffer = vec![0u16; size as usize + 1];
    size = buffer.len() as u32;
    // SAFETY: The buffer holds `size` wide characters.
    if unsafe { GetComputerNameExW(ComputerNameDnsFullyQualified, buffer.as_mut_ptr(), &mut size) } == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..size as usize]))
}
