//! Download PAC scripts using HTTP.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::Error;
use crate::util::{SCRIPT_MAX, split_host_port, strip_ipv6_brackets, url_host, url_path, url_scheme};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const HEADER_MAX: usize = 64 * 1024;

fn fetch_error(url: &str, reason: impl std::fmt::Display) -> Error {
    Error::Fetch(format!("{url}: {reason}"))
}

// Find the end of the response headers
fn find_header_end(response: &[u8]) -> Option<usize> {
    response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|i| i + 4)
}

// Parse the status code and content length from the response headers
fn parse_headers(headers: &str) -> Option<(u16, Option<usize>, bool)> {
    let mut lines = headers.split("\r\n");
    let status = lines.next()?.split_whitespace().nth(1)?.parse().ok()?;
    let mut content_length = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.trim().eq_ignore_ascii_case("content-length") {
            content_length = Some(value.parse().ok()?);
        } else if name.trim().eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.to_ascii_lowercase().contains("chunked");
        }
    }
    Some((status, content_length, chunked))
}

// Connect to the first reachable address for the host
fn connect(host: &str, port: u16) -> std::io::Result<TcpStream> {
    let mut last_error = std::io::Error::new(std::io::ErrorKind::NotFound, "no addresses found");
    for address in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

/// Downloads a PAC script.
pub(crate) fn fetch_get(url: &str) -> Result<String, Error> {
    #[cfg(feature = "https")]
    return fetch_ureq(url);
    #[cfg(not(feature = "https"))]
    fetch_http(url)
}

/// Downloads a PAC script using HTTP or HTTPS, following redirects.
#[cfg(feature = "https")]
fn fetch_ureq(url: &str) -> Result<String, Error> {
    use ureq::tls::{RootCerts, TlsConfig};

    let agent: ureq::Agent = ureq::Agent::config_builder()
        // Never download the PAC script through a proxy
        .proxy(None)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(Some(CONNECT_TIMEOUT + IO_TIMEOUT))
        .tls_config(TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build())
        .build()
        .into();
    let mut response = agent
        .get(url)
        .header("Accept", "application/x-ns-proxy-autoconfig")
        .call()
        .map_err(|error| fetch_error(url, error))?;
    let body = response
        .body_mut()
        .with_config()
        .limit(SCRIPT_MAX as u64)
        .read_to_vec()
        .map_err(|error| fetch_error(url, error))?;
    if body.is_empty() {
        return Err(fetch_error(url, "empty script"));
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Downloads a PAC script using HTTP 1.0.
#[cfg_attr(feature = "https", allow(dead_code))]
fn fetch_http(url: &str) -> Result<String, Error> {
    if !url_scheme(url).unwrap_or("http").eq_ignore_ascii_case("http") {
        return Err(fetch_error(url, "only http is supported"));
    }
    let authority = url_host(url);
    let (host, port) = split_host_port(authority);
    let host = strip_ipv6_brackets(host);
    if host.is_empty() {
        return Err(fetch_error(url, "missing host"));
    }

    let mut stream = connect(host, port.unwrap_or(80)).map_err(|error| fetch_error(url, error))?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .and_then(|_| stream.set_write_timeout(Some(IO_TIMEOUT)))
        .map_err(|error| fetch_error(url, error))?;

    // Create http request using bare-minimum headers
    let request = format!(
        "GET {} HTTP/1.0\r\nHost: {authority}\r\nAccept: application/x-ns-proxy-autoconfig\r\nConnection: close\r\n\r\n",
        url_path(url)
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| fetch_error(url, error))?;

    // Read the response until the connection is closed or the limit is reached
    let limit = (HEADER_MAX + SCRIPT_MAX + 1) as u64;
    let mut response = Vec::new();
    let mut reader = (&mut stream).take(limit);
    let mut header_end = None;
    let mut buffer = [0u8; 8192];
    loop {
        let count = reader.read(&mut buffer).map_err(|error| fetch_error(url, error))?;
        if count == 0 {
            break;
        }
        response.extend_from_slice(&buffer[..count]);
        if header_end.is_none() {
            header_end = find_header_end(&response);
            if header_end.is_none() && response.len() > HEADER_MAX {
                return Err(fetch_error(url, "response headers too large"));
            }
        }
        // Stop once the full body has been received
        if let Some(end) = header_end {
            let headers = String::from_utf8_lossy(&response[..end]);
            if let Some((_, Some(content_length), _)) = parse_headers(&headers) {
                if response.len() - end >= content_length {
                    break;
                }
            }
        }
    }

    let header_end = header_end.ok_or_else(|| fetch_error(url, "incomplete response headers"))?;
    let headers = String::from_utf8_lossy(&response[..header_end]);
    let (status, content_length, chunked) =
        parse_headers(&headers).ok_or_else(|| fetch_error(url, "invalid response headers"))?;
    if status != 200 {
        return Err(fetch_error(url, format!("status code {status}")));
    }
    if chunked {
        return Err(fetch_error(url, "chunked transfer encoding is not supported"));
    }

    let mut body = &response[header_end..];
    if let Some(content_length) = content_length {
        if body.len() < content_length {
            return Err(fetch_error(url, "response body is truncated"));
        }
        body = &body[..content_length];
    }
    if body.is_empty() || body.len() > SCRIPT_MAX {
        return Err(fetch_error(url, format!("invalid script size {}", body.len())));
    }
    Ok(String::from_utf8_lossy(body).into_owned())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    /// Serves a single canned response on a local port and returns the url to request.
    pub(crate) fn serve_once(response: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(&response);
            // Drain before closing so unread data does not reset the connection
            let _ = stream.shutdown(std::net::Shutdown::Write);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let _ = std::io::copy(&mut stream, &mut std::io::sink());
        });
        format!("http://127.0.0.1:{port}/wpad.dat")
    }

    fn response(status: &str, headers: &str, body: &str) -> Vec<u8> {
        format!("HTTP/1.0 {status}\r\n{headers}\r\n{body}").into_bytes()
    }

    #[test]
    fn get() {
        let body = "function FindProxyForURL(url, host) { return 'DIRECT'; }";
        let url = serve_once(response("200 OK", &format!("Content-Length: {}\r\n", body.len()), body));
        assert_eq!(fetch_http(&url).as_deref(), Ok(body));
    }

    #[test]
    fn get_without_content_length() {
        let body = "function FindProxyForURL(url, host) { return 'DIRECT'; }";
        let url = serve_once(response("200 OK", "", body));
        assert_eq!(fetch_http(&url).as_deref(), Ok(body));
    }

    #[test]
    fn get_large_body() {
        let body = "x".repeat(100 * 1024);
        let url = serve_once(response("200 OK", &format!("content-length:{}\r\n", body.len()), &body));
        assert_eq!(fetch_http(&url).map(|script| script.len()), Ok(body.len()));
    }

    #[test]
    fn body_longer_than_content_length() {
        let url = serve_once(response("200 OK", "Content-Length: 4\r\n", &"function".repeat(100)));
        assert_eq!(fetch_http(&url).as_deref(), Ok("func"));
    }

    #[test]
    fn body_shorter_than_content_length() {
        let url = serve_once(response("200 OK", "Content-Length: 100\r\n", "function"));
        assert!(matches!(fetch_http(&url), Err(Error::Fetch(_))));
    }

    #[test]
    fn not_found() {
        let url = serve_once(response("404 Not Found", "Content-Length: 9\r\n", "not found"));
        assert!(matches!(fetch_http(&url), Err(Error::Fetch(message)) if message.contains("404")));
    }

    #[test]
    fn too_large() {
        let body = "x".repeat(SCRIPT_MAX + 1);
        let url = serve_once(response("200 OK", "", &body));
        assert!(matches!(fetch_http(&url), Err(Error::Fetch(_))));
    }

    #[test]
    fn https_not_supported() {
        assert!(matches!(fetch_http("https://127.0.0.1/wpad.dat"), Err(Error::Fetch(_))));
    }

    #[test]
    fn connection_refused() {
        // Bind and drop a listener to find a port that is not listening
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert!(matches!(
            fetch_http(&format!("http://127.0.0.1:{port}/")),
            Err(Error::Fetch(_))
        ));
    }

    #[cfg(feature = "https")]
    mod https {
        use super::*;

        #[test]
        fn get() {
            let body = "function FindProxyForURL(url, host) { return 'DIRECT'; }";
            let url = serve_once(response("200 OK", &format!("Content-Length: {}\r\n", body.len()), body));
            assert_eq!(fetch_get(&url).as_deref(), Ok(body));
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
            assert!(matches!(fetch_get(&url), Err(Error::Fetch(_))));
        }

        #[test]
        fn get_https() {
            let body = fetch_get("https://example.com/").expect("unable to fetch https url");
            assert!(body.contains("Example Domain"));
        }
    }
}
