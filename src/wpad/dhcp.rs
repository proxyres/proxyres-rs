//! WPAD url discovery by sending a DHCPINFORM request.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant, SystemTime};

use super::trim_wpad_url;
use crate::net_adapter::NetAdapter;

const DHCP_SERVER_PORT: u16 = 67;
const DHCP_CLIENT_PORT: u16 = 68;

const DHCP_MAGIC: [u8; 4] = [0x63, 0x82, 0x53, 0x63];

const DHCP_BOOT_REQUEST: u8 = 1;
const DHCP_BOOT_REPLY: u8 = 2;

const DHCP_ACK: u8 = 5;
const DHCP_INFORM: u8 = 8;

const DHCP_OPT_PAD: u8 = 0;
const DHCP_OPT_MSGTYPE: u8 = 0x35;
const DHCP_OPT_PARAMREQ: u8 = 0x37;
const DHCP_OPT_WPAD: u8 = 0xfc;
const DHCP_OPT_END: u8 = 0xff;

const ETHERNET_TYPE: u8 = 1;

// Offsets of fields in the fixed size DHCP header
const XID_OFFSET: usize = 4;
const CIADDR_OFFSET: usize = 12;
const CHADDR_OFFSET: usize = 28;
const CHADDR_LENGTH: usize = 16;
const OPTIONS_OFFSET: usize = 236;

// Minimum BOOTP message size
const MESSAGE_MIN_LENGTH: usize = 300;

/// Builds a DHCPINFORM request asking for the WPAD option.
pub(super) fn build_inform(xid: u32, mac: &[u8], ip: Ipv4Addr) -> Vec<u8> {
    let mut request = vec![0u8; OPTIONS_OFFSET];
    let mac = &mac[..mac.len().min(CHADDR_LENGTH)];
    request[0] = DHCP_BOOT_REQUEST;
    request[1] = ETHERNET_TYPE;
    request[2] = mac.len() as u8;
    request[XID_OFFSET..XID_OFFSET + 4].copy_from_slice(&xid.to_be_bytes());
    request[CIADDR_OFFSET..CIADDR_OFFSET + 4].copy_from_slice(&ip.octets());
    request[CHADDR_OFFSET..CHADDR_OFFSET + mac.len()].copy_from_slice(mac);

    request.extend_from_slice(&DHCP_MAGIC);
    request.extend_from_slice(&[DHCP_OPT_MSGTYPE, 1, DHCP_INFORM]);
    request.extend_from_slice(&[DHCP_OPT_PARAMREQ, 1, DHCP_OPT_WPAD]);
    request.push(DHCP_OPT_END);
    request.resize(request.len().max(MESSAGE_MIN_LENGTH), DHCP_OPT_PAD);
    request
}

// Find an option in the options section of a DHCP message
fn find_option(options: &[u8], option_type: u8) -> Option<&[u8]> {
    let mut i = 0;
    while i < options.len() {
        match options[i] {
            DHCP_OPT_PAD => i += 1,
            DHCP_OPT_END => break,
            current_type => {
                let length = usize::from(*options.get(i + 1)?);
                let value = options.get(i + 2..i + 2 + length)?;
                if current_type == option_type {
                    return Some(value);
                }
                i += 2 + length;
            }
        }
    }
    None
}

/// Parses a DHCP reply to a request and returns its WPAD url.
pub(super) fn parse_reply(reply: &[u8], xid: u32) -> Result<Option<String>, &'static str> {
    if reply.len() < OPTIONS_OFFSET + DHCP_MAGIC.len() {
        return Err("reply too short");
    }
    if reply[0] != DHCP_BOOT_REPLY {
        return Err("not a reply");
    }
    if reply[XID_OFFSET..XID_OFFSET + 4] != xid.to_be_bytes() {
        return Err("transaction id mismatch");
    }
    if reply[OPTIONS_OFFSET..OPTIONS_OFFSET + 4] != DHCP_MAGIC {
        return Err("invalid magic");
    }
    let options = &reply[OPTIONS_OFFSET + 4..];
    if find_option(options, DHCP_OPT_MSGTYPE) != Some(&[DHCP_ACK]) {
        return Err("not an acknowledgement");
    }
    Ok(find_option(options, DHCP_OPT_WPAD).and_then(trim_wpad_url))
}

// Generate a random transaction id
fn random_xid() -> u32 {
    let mut hasher = RandomState::new().build_hasher();
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    hasher.write_u128(now.as_nanos());
    hasher.finish() as u32
}

/// Sends a DHCPINFORM request to the target address and waits for the WPAD url in the reply.
pub(super) fn retrieve_wpad(bind_ip: Ipv4Addr, mac: &[u8], target: SocketAddr, timeout: Duration) -> Option<String> {
    let socket = match UdpSocket::bind(SocketAddrV4::new(bind_ip, DHCP_CLIENT_PORT)) {
        Ok(socket) => socket,
        // Likely can't bind to the protected client port, try again with a random port
        Err(error) if matches!(error.kind(), ErrorKind::PermissionDenied | ErrorKind::AddrInUse) => {
            UdpSocket::bind(SocketAddrV4::new(bind_ip, 0)).ok()?
        }
        Err(error) => {
            log::debug!("Unable to bind udp socket ({error})");
            return None;
        }
    };
    socket.set_broadcast(true).ok()?;

    let xid = random_xid();
    let request = build_inform(xid, mac, bind_ip);
    if let Err(error) = socket.send_to(&request, target) {
        log::debug!("Unable to send DHCP inform ({error})");
        return None;
    }

    // Ignore unrelated replies until the timeout
    let deadline = Instant::now() + timeout;
    let mut reply = [0u8; 1500];
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())?;
        socket.set_read_timeout(Some(remaining)).ok()?;
        let Ok((length, _)) = socket.recv_from(&mut reply) else {
            return None;
        };
        match parse_reply(&reply[..length], xid) {
            Ok(wpad) => return wpad,
            Err(reason) => log::debug!("Ignoring DHCP reply ({reason})"),
        }
    }
}

/// Gets the WPAD url by broadcasting a DHCPINFORM request from the adapter.
pub(super) fn wpad_dhcp_adapter(adapter: &NetAdapter, timeout: Duration) -> Option<String> {
    let ip = adapter.ipv4()?;
    let target = SocketAddr::from((Ipv4Addr::BROADCAST, DHCP_SERVER_PORT));
    retrieve_wpad(ip, &adapter.mac, target, timeout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    // Build a reply to a request with the given options
    fn build_reply(request: &[u8], options: &[u8]) -> Vec<u8> {
        let mut reply = request[..OPTIONS_OFFSET + 4].to_vec();
        reply[0] = DHCP_BOOT_REPLY;
        reply.extend_from_slice(options);
        reply
    }

    type OptionCase<'a> = (&'a [u8], u8, Option<&'a [u8]>);

    #[test]
    fn find_option_cases() {
        let wpad = DHCP_OPT_WPAD;
        let tests: [OptionCase; 8] = [
            (&[DHCP_OPT_MSGTYPE, 1, DHCP_ACK, DHCP_OPT_END], wpad, None),
            (
                &[DHCP_OPT_MSGTYPE, 1, DHCP_ACK, DHCP_OPT_END],
                DHCP_OPT_MSGTYPE,
                Some(&[DHCP_ACK]),
            ),
            // Padding before an option
            (
                &[DHCP_OPT_PAD, DHCP_OPT_PAD, wpad, 1, b'x', DHCP_OPT_END],
                wpad,
                Some(b"x"),
            ),
            // No end marker
            (&[DHCP_OPT_MSGTYPE, 1, DHCP_ACK], wpad, None),
            (&[DHCP_OPT_MSGTYPE, 1, DHCP_ACK], DHCP_OPT_MSGTYPE, Some(&[DHCP_ACK])),
            // Truncated at the length or value
            (&[wpad], wpad, None),
            (&[wpad, 10, b'x'], wpad, None),
            // Truncated option before the requested option
            (&[DHCP_OPT_MSGTYPE, 200, DHCP_ACK, wpad, 1, b'x'], wpad, None),
        ];
        for (options, option_type, expected) in tests {
            assert_eq!(find_option(options, option_type), expected, "options: {options:?}");
        }
    }

    #[test]
    fn inform_request() {
        let request = build_inform(0x12345678, &[0xaa; 6], Ipv4Addr::new(192, 168, 1, 5));
        assert_eq!(request.len(), MESSAGE_MIN_LENGTH);
        assert_eq!(request[0], DHCP_BOOT_REQUEST);
        assert_eq!(request[2], 6);
        assert_eq!(&request[XID_OFFSET..XID_OFFSET + 4], &[0x12, 0x34, 0x56, 0x78]);
        assert_eq!(&request[CIADDR_OFFSET..CIADDR_OFFSET + 4], &[192, 168, 1, 5]);
        assert_eq!(&request[OPTIONS_OFFSET..OPTIONS_OFFSET + 4], &DHCP_MAGIC);
        let options = &request[OPTIONS_OFFSET + 4..];
        assert_eq!(find_option(options, DHCP_OPT_MSGTYPE), Some(&[DHCP_INFORM][..]));
        assert_eq!(find_option(options, DHCP_OPT_PARAMREQ), Some(&[DHCP_OPT_WPAD][..]));
    }

    #[test]
    fn reply_with_wpad() {
        let request = build_inform(1, &[0xaa; 6], Ipv4Addr::LOCALHOST);
        let mut options = vec![DHCP_OPT_MSGTYPE, 1, DHCP_ACK, DHCP_OPT_PAD, DHCP_OPT_WPAD, 25];
        options.extend_from_slice(b"http://wpad.com/wpad.dat\n");
        options.push(DHCP_OPT_END);
        let reply = build_reply(&request, &options);
        assert_eq!(parse_reply(&reply, 1), Ok(Some("http://wpad.com/wpad.dat".to_string())));
        assert_eq!(parse_reply(&reply, 2), Err("transaction id mismatch"));
    }

    #[test]
    fn reply_without_msgtype() {
        let request = build_inform(1, &[0xaa; 6], Ipv4Addr::LOCALHOST);
        let reply = build_reply(&request, &[DHCP_OPT_END]);
        assert_eq!(parse_reply(&reply, 1), Err("not an acknowledgement"));
    }

    #[test]
    fn reply_with_truncated_option() {
        let request = build_inform(1, &[0xaa; 6], Ipv4Addr::LOCALHOST);
        let reply = build_reply(&request, &[DHCP_OPT_MSGTYPE, 1, DHCP_ACK, DHCP_OPT_WPAD, 255, b'h']);
        assert_eq!(parse_reply(&reply, 1), Ok(None));
        let reply = build_reply(&request, &[DHCP_OPT_MSGTYPE]);
        assert_eq!(parse_reply(&reply, 1), Err("not an acknowledgement"));
    }

    #[test]
    fn reply_too_short() {
        assert_eq!(parse_reply(&[DHCP_BOOT_REPLY; 10], 1), Err("reply too short"));
    }

    #[test]
    fn reply_with_wrong_op_or_magic() {
        let request = build_inform(1, &[0xaa; 6], Ipv4Addr::LOCALHOST);
        let options = [DHCP_OPT_MSGTYPE, 1, DHCP_ACK, DHCP_OPT_END];
        let mut reply = build_reply(&request, &options);
        reply[0] = DHCP_BOOT_REQUEST;
        assert_eq!(parse_reply(&reply, 1), Err("not a reply"));
        let mut reply = build_reply(&request, &options);
        reply[OPTIONS_OFFSET] ^= 0xff;
        assert_eq!(parse_reply(&reply, 1), Err("invalid magic"));
    }

    #[test]
    fn request_to_local_server() {
        // Fake DHCP server that ignores the first request and answers with a WPAD url
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let target = server.local_addr().unwrap();
        thread::spawn(move || {
            let mut request = [0u8; 1500];
            let (length, client) = server.recv_from(&mut request).unwrap();
            let request = &request[..length];
            let mut unrelated = build_reply(request, &[DHCP_OPT_MSGTYPE, 1, DHCP_ACK, DHCP_OPT_END]);
            unrelated[XID_OFFSET] ^= 0xff;
            server.send_to(&unrelated, client).unwrap();
            let mut options = vec![DHCP_OPT_MSGTYPE, 1, DHCP_ACK, DHCP_OPT_WPAD, 24];
            options.extend_from_slice(b"http://wpad.com/wpad.dat");
            options.push(DHCP_OPT_END);
            server.send_to(&build_reply(request, &options), client).unwrap();
        });
        let wpad = retrieve_wpad(Ipv4Addr::LOCALHOST, &[0xaa; 6], target, Duration::from_secs(5));
        assert_eq!(wpad.as_deref(), Some("http://wpad.com/wpad.dat"));
    }

    #[test]
    fn request_timeout() {
        // Server that never answers
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let target = server.local_addr().unwrap();
        let start = Instant::now();
        assert_eq!(
            retrieve_wpad(Ipv4Addr::LOCALHOST, &[0xaa; 6], target, Duration::from_millis(300)),
            None
        );
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}
