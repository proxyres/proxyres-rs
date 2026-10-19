//! Core Foundation helpers.

use std::ffi::{CStr, c_char, c_void};

#[cfg(target_os = "macos")]
use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetTypeID, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation_sys::base::{CFGetTypeID, CFIndex, CFRelease, CFTypeRef, kCFAllocatorDefault};
use core_foundation_sys::dictionary::{CFDictionaryGetValueIfPresent, CFDictionaryRef};
use core_foundation_sys::number::{CFNumberGetTypeID, CFNumberGetValue, CFNumberRef, kCFNumberSInt64Type};
use core_foundation_sys::string::{
    CFStringCreateWithBytes, CFStringGetCString, CFStringGetCStringPtr, CFStringGetLength,
    CFStringGetMaximumSizeForEncoding, CFStringGetTypeID, CFStringRef, kCFStringEncodingUTF8,
};
use core_foundation_sys::url::CFURLCreateWithBytes;

/// Core Foundation object released when dropped.
pub(crate) struct CfOwned(CFTypeRef);

impl CfOwned {
    /// Takes ownership of an object returned by a create or copy function.
    pub(crate) fn new(object: CFTypeRef) -> Option<Self> {
        (!object.is_null()).then_some(CfOwned(object))
    }

    pub(crate) fn as_ptr(&self) -> CFTypeRef {
        self.0
    }
}

impl Drop for CfOwned {
    fn drop(&mut self) {
        // SAFETY: The object is owned by this wrapper.
        unsafe { CFRelease(self.0) };
    }
}

/// Creates a string from a Rust string.
pub(crate) fn cf_string_create(s: &str) -> Option<CfOwned> {
    // SAFETY: The bytes are valid for the duration of the call and the string is owned by the caller.
    CfOwned::new(unsafe {
        CFStringCreateWithBytes(
            kCFAllocatorDefault,
            s.as_ptr(),
            s.len() as CFIndex,
            kCFStringEncodingUTF8,
            0,
        )
    } as CFTypeRef)
}

/// Creates a url from a Rust string.
pub(crate) fn cf_url_create(url: &str) -> Option<CfOwned> {
    // SAFETY: The bytes are valid for the duration of the call and the url is owned by the caller.
    CfOwned::new(unsafe {
        CFURLCreateWithBytes(
            kCFAllocatorDefault,
            url.as_ptr(),
            url.len() as CFIndex,
            kCFStringEncodingUTF8,
            std::ptr::null(),
        )
    } as CFTypeRef)
}

/// Converts a string to a Rust string.
///
/// # Safety
///
/// The pointer must be null or a valid string.
pub(crate) unsafe fn cf_string_to_string(string: CFStringRef) -> Option<String> {
    if string.is_null() {
        return None;
    }
    // SAFETY: The caller guarantees the string is valid.
    unsafe {
        let ptr = CFStringGetCStringPtr(string, kCFStringEncodingUTF8);
        if !ptr.is_null() {
            return Some(CStr::from_ptr(ptr).to_string_lossy().into_owned());
        }
        let max_size = CFStringGetMaximumSizeForEncoding(CFStringGetLength(string), kCFStringEncodingUTF8) + 1;
        let mut buffer = vec![0u8; usize::try_from(max_size).ok()?];
        if CFStringGetCString(
            string,
            buffer.as_mut_ptr() as *mut c_char,
            max_size,
            kCFStringEncodingUTF8,
        ) == 0
        {
            return None;
        }
        Some(
            CStr::from_ptr(buffer.as_ptr() as *const c_char)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

/// Gets a value from a dictionary if it is of the expected type.
///
/// # Safety
///
/// The dictionary and key must be valid.
pub(crate) unsafe fn cf_dictionary_value(
    dictionary: CFDictionaryRef,
    key: CFStringRef,
    type_id: usize,
) -> Option<CFTypeRef> {
    let mut value: *const c_void = std::ptr::null();
    // SAFETY: The caller guarantees the dictionary and key are valid.
    unsafe {
        if CFDictionaryGetValueIfPresent(dictionary, key as *const c_void, &mut value) == 0 || value.is_null() {
            return None;
        }
        (CFGetTypeID(value) == type_id as _).then_some(value)
    }
}

/// Gets a string value from a dictionary.
///
/// # Safety
///
/// The dictionary and key must be valid.
pub(crate) unsafe fn cf_dictionary_string(dictionary: CFDictionaryRef, key: CFStringRef) -> Option<String> {
    // SAFETY: The caller guarantees the dictionary and key are valid.
    unsafe {
        let value = cf_dictionary_value(dictionary, key, CFStringGetTypeID() as usize)?;
        cf_string_to_string(value as CFStringRef)
    }
}

/// Gets a number value from a dictionary.
///
/// # Safety
///
/// The dictionary and key must be valid.
pub(crate) unsafe fn cf_dictionary_i64(dictionary: CFDictionaryRef, key: CFStringRef) -> Option<i64> {
    // SAFETY: The caller guarantees the dictionary and key are valid.
    unsafe {
        let value = cf_dictionary_value(dictionary, key, CFNumberGetTypeID() as usize)?;
        let mut number: i64 = 0;
        CFNumberGetValue(
            value as CFNumberRef,
            kCFNumberSInt64Type,
            &mut number as *mut i64 as *mut c_void,
        )
        .then_some(number)
    }
}

/// Gets whether a number value in a dictionary is present and non-zero.
///
/// # Safety
///
/// The dictionary and key must be valid.
pub(crate) unsafe fn cf_dictionary_bool(dictionary: CFDictionaryRef, key: CFStringRef) -> bool {
    // SAFETY: The caller guarantees the dictionary and key are valid.
    unsafe { cf_dictionary_i64(dictionary, key).is_some_and(|value| value != 0) }
}

/// Gets an array of strings from a dictionary.
///
/// # Safety
///
/// The dictionary and key must be valid.
#[cfg(target_os = "macos")]
pub(crate) unsafe fn cf_dictionary_strings(dictionary: CFDictionaryRef, key: CFStringRef) -> Vec<String> {
    // SAFETY: The caller guarantees the dictionary and key are valid.
    unsafe {
        let Some(array) = cf_dictionary_value(dictionary, key, CFArrayGetTypeID() as usize) else {
            return Vec::new();
        };
        let array = array as CFArrayRef;
        (0..CFArrayGetCount(array))
            .filter_map(|i| {
                let value = CFArrayGetValueAtIndex(array, i);
                if value.is_null() || CFGetTypeID(value) != CFStringGetTypeID() {
                    return None;
                }
                cf_string_to_string(value as CFStringRef)
            })
            .collect()
    }
}
