//! Windows string helpers.

/// Converts a string into a null-terminated wide string.
pub(crate) fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Converts a null-terminated wide string into a string.
///
/// # Safety
///
/// The pointer must be null or point to a null-terminated wide string.
pub(crate) unsafe fn wide_ptr_to_string(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: The caller guarantees the string is null-terminated.
    unsafe {
        let len = (0..).take_while(|&i| *ptr.add(i) != 0).count();
        String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let wide = to_wide("proxy:80");
        assert_eq!(wide.last(), Some(&0));
        assert_eq!(unsafe { wide_ptr_to_string(wide.as_ptr()) }, "proxy:80");
        assert_eq!(unsafe { wide_ptr_to_string(std::ptr::null()) }, "");
    }
}
