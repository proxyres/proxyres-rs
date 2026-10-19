//! Download PAC scripts using CFNetwork, which supports HTTPS.

use std::cell::RefCell;
use std::ffi::c_void;
use std::time::Instant;

use core_foundation_sys::base::{CFIndex, CFTypeRef, kCFAllocatorDefault};
use core_foundation_sys::dictionary::{
    CFDictionaryCreate, kCFTypeDictionaryKeyCallBacks, kCFTypeDictionaryValueCallBacks,
};
use core_foundation_sys::error::{CFErrorCopyDescription, CFErrorRef};
use core_foundation_sys::number::kCFBooleanFalse;
use core_foundation_sys::runloop::{CFRunLoopGetCurrent, CFRunLoopRunInMode, kCFRunLoopRunFinished};
use core_foundation_sys::stream::{
    CFReadStreamClose, CFReadStreamCopyError, CFReadStreamCopyProperty, CFReadStreamOpen, CFReadStreamRead,
    CFReadStreamRef, CFReadStreamScheduleWithRunLoop, CFReadStreamSetClient, CFReadStreamSetProperty,
    CFReadStreamUnscheduleFromRunLoop, CFStreamClientContext, CFStreamEventType, kCFStreamEventEndEncountered,
    kCFStreamEventErrorOccurred, kCFStreamEventHasBytesAvailable,
};
use core_foundation_sys::string::CFStringRef;
use core_foundation_sys::url::{CFURLCopyAbsoluteURL, CFURLCreateWithString, CFURLGetString, CFURLRef};

use super::{CONNECT_TIMEOUT, IO_TIMEOUT, fetch_error};
use crate::Error;
use crate::macos::cf::{CfOwned, cf_string_create, cf_string_to_string, cf_url_create};
use crate::macos::cf_network::{
    CFHTTPMessageCopyHeaderFieldValue, CFHTTPMessageCreateRequest, CFHTTPMessageGetResponseStatusCode,
    CFHTTPMessageRef, CFHTTPMessageSetHeaderFieldValue, CFReadStreamCreateForHTTPRequest, StreamClientContext,
    kCFHTTPVersion1_1, kCFStreamPropertyHTTPProxy, kCFStreamPropertyHTTPResponseHeader,
    kCFStreamPropertyHTTPShouldAutoredirect,
};
use crate::util::{SCRIPT_MAX, url_scheme};

const MAX_REDIRECTS: usize = 10;

// Body of a response and how it finished, updated by the stream callback
#[derive(Default)]
struct Transfer {
    body: Vec<u8>,
    finished: Option<Result<(), String>>,
}

enum Response {
    Script(Vec<u8>),
    Redirect(String),
}

fn is_scheme(url: &str, scheme: &str) -> bool {
    url_scheme(url).is_some_and(|url_scheme| url_scheme.eq_ignore_ascii_case(scheme))
}

// Get the description of the stream's error
unsafe fn stream_error(stream: CFReadStreamRef) -> String {
    // SAFETY: The caller passes a valid stream, and the copied error and description are owned here.
    unsafe {
        CfOwned::new(CFReadStreamCopyError(stream) as CFTypeRef)
            .and_then(|error| CfOwned::new(CFErrorCopyDescription(error.as_ptr() as CFErrorRef) as CFTypeRef))
            .and_then(|description| cf_string_to_string(description.as_ptr() as CFStringRef))
            .unwrap_or_else(|| "stream error".to_string())
    }
}

extern "C" fn stream_callback(stream: CFReadStreamRef, event: CFStreamEventType, info: *mut c_void) {
    // SAFETY: The info is the transfer owned by request on this thread, which closes the stream
    // before the transfer goes out of scope.
    let mut transfer = unsafe { &*(info as *const RefCell<Transfer>) }.borrow_mut();
    if transfer.finished.is_some() {
        return;
    }
    if event == kCFStreamEventHasBytesAvailable {
        let mut buffer = [0u8; 8192];
        // SAFETY: The stream is valid during the callback and the buffer is writable.
        let count = unsafe { CFReadStreamRead(stream, buffer.as_mut_ptr(), buffer.len() as CFIndex) };
        if count > 0 {
            transfer.body.extend_from_slice(&buffer[..count as usize]);
            if transfer.body.len() > SCRIPT_MAX {
                transfer.finished = Some(Err("script exceeds maximum size".to_string()));
            }
        }
    } else if event == kCFStreamEventEndEncountered {
        transfer.finished = Some(Ok(()));
    } else if event == kCFStreamEventErrorOccurred {
        // SAFETY: The stream is valid during the callback.
        transfer.finished = Some(Err(unsafe { stream_error(stream) }));
    }
}

// Get the absolute url of a redirect location relative to the requested url
unsafe fn redirect_url(response: CFHTTPMessageRef, url: CFURLRef) -> Option<String> {
    let header = cf_string_create("Location")?;
    // SAFETY: The caller passes a valid response and url, and every copy or create result is owned.
    unsafe {
        let location =
            CfOwned::new(CFHTTPMessageCopyHeaderFieldValue(response, header.as_ptr() as CFStringRef) as CFTypeRef)?;
        let relative = CfOwned::new(
            CFURLCreateWithString(kCFAllocatorDefault, location.as_ptr() as CFStringRef, url) as CFTypeRef,
        )?;
        let absolute = CfOwned::new(CFURLCopyAbsoluteURL(relative.as_ptr() as CFURLRef) as CFTypeRef)?;
        cf_string_to_string(CFURLGetString(absolute.as_ptr() as CFURLRef))
    }
}

// Request a url once on the current thread's run loop, without following redirects
fn request(url: &str, deadline: Instant) -> Result<Response, String> {
    let cf_url = cf_url_create(url).ok_or("invalid url")?;
    let method = cf_string_create("GET").ok_or("out of memory")?;
    let accept = cf_string_create("Accept").ok_or("out of memory")?;
    let accept_value = cf_string_create("application/x-ns-proxy-autoconfig").ok_or("out of memory")?;
    let mode = cf_string_create("proxyres.fetch").ok_or("out of memory")?;
    // SAFETY: The method, url, and version are valid, and the request is owned.
    let message = CfOwned::new(unsafe {
        CFHTTPMessageCreateRequest(
            kCFAllocatorDefault,
            method.as_ptr() as CFStringRef,
            cf_url.as_ptr() as CFURLRef,
            kCFHTTPVersion1_1.as_ptr(),
        )
    } as CFTypeRef)
    .ok_or("unable to create request")?;
    // SAFETY: The request and strings are valid, and the stream is owned.
    let owned_stream = CfOwned::new(unsafe {
        CFHTTPMessageSetHeaderFieldValue(
            message.as_ptr() as CFHTTPMessageRef,
            accept.as_ptr() as CFStringRef,
            accept_value.as_ptr() as CFStringRef,
        );
        CFReadStreamCreateForHTTPRequest(kCFAllocatorDefault, message.as_ptr() as CFHTTPMessageRef)
    } as CFTypeRef)
    .ok_or("unable to create stream")?;
    let stream = owned_stream.as_ptr() as CFReadStreamRef;
    // SAFETY: No keys or values are read for an empty dictionary, which is owned.
    let no_proxy = CfOwned::new(unsafe {
        CFDictionaryCreate(
            kCFAllocatorDefault,
            std::ptr::null(),
            std::ptr::null(),
            0,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    } as CFTypeRef)
    .ok_or("out of memory")?;

    let transfer = RefCell::new(Transfer::default());
    let mut context = StreamClientContext {
        version: 0,
        info: &transfer as *const _ as *mut c_void,
        retain: None,
        release: None,
        copy_description: None,
    };
    // SAFETY: The stream, context, and mode are valid, and the stream is closed before the
    // transfer and context are dropped.
    let response = unsafe {
        // Redirects are followed by fetch_get so their scheme can be checked
        CFReadStreamSetProperty(
            stream,
            kCFStreamPropertyHTTPShouldAutoredirect.as_ptr(),
            kCFBooleanFalse as CFTypeRef,
        );
        // Download the PAC script directly, since the proxy can depend on the script itself
        CFReadStreamSetProperty(stream, kCFStreamPropertyHTTPProxy.as_ptr(), no_proxy.as_ptr());
        let events = kCFStreamEventHasBytesAvailable | kCFStreamEventEndEncountered | kCFStreamEventErrorOccurred;
        CFReadStreamSetClient(
            stream,
            events,
            stream_callback,
            &mut context as *mut StreamClientContext as *mut CFStreamClientContext,
        );
        let run_loop = CFRunLoopGetCurrent();
        let mode = mode.as_ptr() as CFStringRef;
        CFReadStreamScheduleWithRunLoop(stream, run_loop, mode);
        if CFReadStreamOpen(stream) == 0 {
            transfer.borrow_mut().finished = Some(Err(stream_error(stream)));
        }
        while transfer.borrow().finished.is_none() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() || CFRunLoopRunInMode(mode, remaining.as_secs_f64(), 1) == kCFRunLoopRunFinished {
                break;
            }
        }
        let response = CfOwned::new(CFReadStreamCopyProperty(
            stream,
            kCFStreamPropertyHTTPResponseHeader.as_ptr(),
        ));
        CFReadStreamUnscheduleFromRunLoop(stream, run_loop, mode);
        CFReadStreamClose(stream);
        response
    };

    let transfer = transfer.into_inner();
    transfer.finished.ok_or("timed out")??;
    let response = response.ok_or("missing response headers")?;
    // SAFETY: The response is a valid HTTP message.
    let status = unsafe { CFHTTPMessageGetResponseStatusCode(response.as_ptr() as CFHTTPMessageRef) };
    match status {
        200..=299 => Ok(Response::Script(transfer.body)),
        301 | 302 | 303 | 307 | 308 => {
            // SAFETY: The response and url are valid.
            unsafe { redirect_url(response.as_ptr() as CFHTTPMessageRef, cf_url.as_ptr() as CFURLRef) }
                .map(Response::Redirect)
                .ok_or_else(|| format!("status code {status} without a valid location"))
        }
        _ => Err(format!("status code {status}")),
    }
}

/// Downloads a PAC script using HTTP or HTTPS, following redirects.
pub(super) fn fetch_get(url: &str) -> Result<String, Error> {
    let deadline = Instant::now() + CONNECT_TIMEOUT + IO_TIMEOUT;
    let mut current = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        // The PAC url can come from DHCP, so only http and https urls are downloaded
        if !is_scheme(&current, "http") && !is_scheme(&current, "https") {
            return Err(fetch_error(&current, "only http and https are supported"));
        }
        match request(&current, deadline).map_err(|reason| fetch_error(&current, reason))? {
            Response::Script(body) if body.is_empty() => return Err(fetch_error(&current, "empty script")),
            Response::Script(body) => return Ok(String::from_utf8_lossy(&body).into_owned()),
            Response::Redirect(location) => {
                if is_scheme(&current, "https") && is_scheme(&location, "http") {
                    return Err(fetch_error(&current, format!("unsafe redirect to {location}")));
                }
                current = location;
            }
        }
    }
    Err(fetch_error(url, "too many redirects"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::tests::{response, serve_once};

    #[test]
    fn get() {
        let body = "function FindProxyForURL(url, host) { return 'DIRECT'; }";
        let url = serve_once(response("200 OK", &format!("Content-Length: {}\r\n", body.len()), body));
        assert_eq!(fetch_get(&url).as_deref(), Ok(body));
    }

    #[test]
    fn get_without_content_length() {
        let body = "function FindProxyForURL(url, host) { return 'DIRECT'; }";
        let url = serve_once(response("200 OK", "", body));
        assert_eq!(fetch_get(&url).as_deref(), Ok(body));
    }

    #[test]
    fn redirect() {
        let body = "function FindProxyForURL(url, host) { return 'DIRECT'; }";
        let target = serve_once(response("200 OK", &format!("Content-Length: {}\r\n", body.len()), body));
        let url = serve_once(response(
            "302 Found",
            &format!("Location: {target}\r\nContent-Length: 0\r\n"),
            "",
        ));
        assert_eq!(fetch_get(&url).as_deref(), Ok(body));
    }

    #[test]
    fn redirect_to_file() {
        let url = serve_once(response(
            "302 Found",
            "Location: file:///etc/hosts\r\nContent-Length: 0\r\n",
            "",
        ));
        assert!(matches!(fetch_get(&url), Err(Error::Fetch(message)) if message.contains("only http and https")));
    }

    #[test]
    fn not_found() {
        let url = serve_once(response("404 Not Found", "Content-Length: 9\r\n", "not found"));
        assert!(matches!(fetch_get(&url), Err(Error::Fetch(message)) if message.contains("404")));
    }

    #[test]
    fn too_large() {
        let body = "x".repeat(SCRIPT_MAX + 1);
        let url = serve_once(response("200 OK", "", &body));
        assert!(matches!(fetch_get(&url), Err(Error::Fetch(message)) if message.contains("maximum size")));
    }

    #[test]
    fn empty() {
        let url = serve_once(response("200 OK", "Content-Length: 0\r\n", ""));
        assert!(matches!(fetch_get(&url), Err(Error::Fetch(message)) if message.contains("empty")));
    }

    #[test]
    fn unsupported_scheme() {
        assert!(matches!(fetch_get("file:///etc/hosts"), Err(Error::Fetch(_))));
        assert!(matches!(fetch_get("wpad.example.com/wpad.dat"), Err(Error::Fetch(_))));
    }

    #[test]
    fn connection_refused() {
        // Bind and drop a listener to find a port that is not listening
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        assert!(matches!(
            fetch_get(&format!("http://127.0.0.1:{port}/")),
            Err(Error::Fetch(_))
        ));
    }

    #[test]
    fn get_https() {
        let body = fetch_get("https://example.com/").expect("unable to fetch https url");
        assert!(body.contains("Example Domain"));
    }
}
