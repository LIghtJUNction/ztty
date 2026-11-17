//! Bounded, single-threaded Streamable HTTP transport (JSON response mode).
use alloc::ffi::CString;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::ffi::{CStr, c_char, c_int, c_void};

use super::{RpcSession, VERSIONS, dispatch, error};
use crate::json::{self, Value};
use crate::os;

const MAX_CLIENTS: usize = 64;
const MAX_SESSIONS: usize = 64;
const MAX_HEADERS: usize = 16384;
const MAX_BUFFERED: usize = 4 * 1024 * 1024;
const REQUEST_TIMEOUT: u64 = 10000;
const SESSION_TIMEOUT: u64 = 30 * 60 * 1000;
const SOCK_STREAM: i32 = 1;
const SOCK_NONBLOCK: i32 = 0o4000;
const SOCK_CLOEXEC: i32 = 0o2000000;

#[repr(C)]
struct AddrInfo {
    flags: c_int,
    family: c_int,
    socktype: c_int,
    protocol: c_int,
    addrlen: u32,
    addr: *mut c_void,
    canonname: *mut c_char,
    next: *mut AddrInfo,
}

unsafe extern "C" {
    fn getaddrinfo(
        node: *const c_char,
        service: *const c_char,
        hints: *const AddrInfo,
        result: *mut *mut AddrInfo,
    ) -> c_int;
    fn freeaddrinfo(info: *mut AddrInfo);
    fn gai_strerror(code: c_int) -> *const c_char;
    fn socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int;
    fn setsockopt(fd: c_int, level: c_int, option: c_int, value: *const c_void, len: u32) -> c_int;
    fn bind(fd: c_int, address: *const c_void, len: u32) -> c_int;
    fn getsockname(fd: c_int, address: *mut c_void, len: *mut u32) -> c_int;
    fn listen(fd: c_int, backlog: c_int) -> c_int;
    fn accept4(fd: c_int, address: *mut c_void, len: *mut u32, flags: c_int) -> c_int;
    fn recv(fd: c_int, buffer: *mut c_void, count: usize, flags: c_int) -> isize;
    fn send(fd: c_int, buffer: *const c_void, count: usize, flags: c_int) -> isize;
    fn getrandom(buffer: *mut c_void, count: usize, flags: u32) -> isize;
}

struct Fd(i32);
impl Drop for Fd {
    fn drop(&mut self) {
        unsafe {
            os::close(self.0);
        }
    }
}

struct Client {
    fd: Fd,
    input: Vec<u8>,
    output: Vec<u8>,
    written: usize,
    deadline: u64,
    replied: bool,
    closed: bool,
}

impl Client {
    fn new(fd: i32) -> Self {
        Self {
            fd: Fd(fd),
            input: Vec::new(),
            output: Vec::new(),
            written: 0,
            deadline: os::milliseconds() + REQUEST_TIMEOUT,
            replied: false,
            closed: false,
        }
    }

    fn reply(&mut self, status: u16, body: &[u8], session: Option<&str>) {
        let reason = match status {
            200 => "OK",
            202 => "Accepted",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            406 => "Not Acceptable",
            408 => "Request Timeout",
            411 => "Length Required",
            413 => "Payload Too Large",
            415 => "Unsupported Media Type",
            431 => "Request Header Fields Too Large",
            503 => "Service Unavailable",
            _ => "Internal Server Error",
        };
        let mut header = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n",
            body.len()
        );
        if !body.is_empty() {
            header.push_str("Content-Type: application/json\r\n");
        }
        if status == 405 {
            header.push_str("Allow: POST, DELETE\r\n");
        }
        if status == 401 {
            header.push_str("WWW-Authenticate: Bearer\r\n");
        }
        if let Some(id) = session {
            header.push_str(&format!("Mcp-Session-Id: {id}\r\n"));
        }
        header.push_str("\r\n");
        self.output = header.into_bytes();
        self.output.extend_from_slice(body);
        self.input.clear();
        self.input.shrink_to(8192);
        self.replied = true;
    }
}

struct Session {
    id: String,
    rpc: RpcSession,
    touched: u64,
}

struct Settings<'a> {
    origin: String,
    authorities: Vec<String>,
    local: bool,
    token: Option<&'a str>,
}

pub(super) fn run(host: &str, port: u16, token: Option<&str>) -> Result<i32, String> {
    let host = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host);
    if host.is_empty()
        || host.len() > 253
        || host
            .bytes()
            .any(|b| b.is_ascii_whitespace() || matches!(b, b'/' | b'@' | b'?' | b'#'))
    {
        return Err("invalid HTTP host".into());
    }
    let (listener, actual_port, local) = bind_listener(host, port, token.is_some())?;
    let authority = authority(host, actual_port);
    let mut authorities = alloc::vec![authority.clone()];
    if local {
        for name in ["localhost", "127.0.0.1", "::1"] {
            let name = authority_for(name, actual_port);
            if !authorities.contains(&name) {
                authorities.push(name);
            }
        }
    }
    let settings = Settings {
        origin: format!("mcp/http/{authority}"),
        authorities,
        local,
        token,
    };
    os::print(
        2,
        format!("ztty MCP listening at http://{authority}/mcp\n").as_bytes(),
    );
    let mut clients: Vec<Client> = Vec::new();
    let mut sessions: Vec<Session> = Vec::new();
    loop {
        let now = os::milliseconds();
        clients.retain(|client| {
            !client.closed
                && (!client.replied || client.written < client.output.len())
                && client.deadline > now
        });
        sessions.retain(|session| now.saturating_sub(session.touched) < SESSION_TIMEOUT);
        let mut fds = Vec::with_capacity(clients.len() + 1);
        fds.push(os::PollFd {
            fd: listener.0,
            events: os::POLLIN,
            revents: 0,
        });
        for client in &clients {
            fds.push(os::PollFd {
                fd: client.fd.0,
                events: if client.replied {
                    os::POLLOUT
                } else {
                    os::POLLIN
                },
                revents: 0,
            });
        }
        let timeout = clients
            .iter()
            .map(|c| c.deadline.saturating_sub(now).min(30000) as i32)
            .min()
            .unwrap_or(30000);
        let count = unsafe { os::poll(fds.as_mut_ptr(), fds.len(), timeout) };
        if count < 0 {
            if os::errno() == os::EINTR {
                continue;
            }
            return Err("HTTP poll failed".into());
        }
        for index in 0..clients.len() {
            let events = fds[index + 1].revents;
            if events & os::POLLERR != 0 {
                clients[index].closed = true;
                continue;
            }
            if clients[index].replied && events & os::POLLOUT != 0 {
                let client = &mut clients[index];
                let pending = &client.output[client.written..];
                let count =
                    unsafe { send(client.fd.0, pending.as_ptr().cast(), pending.len(), 0x4000) };
                if count > 0 {
                    client.written += count as usize;
                } else if count == 0 || (os::errno() != 11 && os::errno() != os::EINTR) {
                    client.closed = true;
                }
            } else if !clients[index].replied && events & (os::POLLIN | os::POLLHUP) != 0 {
                for _ in 0..8 {
                    let mut buffer = [0u8; 8192];
                    let count = unsafe {
                        recv(
                            clients[index].fd.0,
                            buffer.as_mut_ptr().cast(),
                            buffer.len(),
                            0,
                        )
                    };
                    if count == 0 {
                        clients[index].closed = true;
                        break;
                    }
                    if count < 0 {
                        if os::errno() == os::EINTR {
                            continue;
                        }
                        if os::errno() != 11 {
                            clients[index].closed = true;
                        }
                        break;
                    }
                    let buffered: usize = clients.iter().map(|c| c.input.len()).sum();
                    if buffered + count as usize > MAX_BUFFERED
                        || clients[index].input.len() + count as usize > json::MAX_MESSAGE + 65536
                    {
                        clients[index].reply(413, &[], None);
                        break;
                    }
                    clients[index]
                        .input
                        .extend_from_slice(&buffer[..count as usize]);
                    match parse_request(&clients[index].input) {
                        Ok(Some(request)) => {
                            let (status, body, id) = respond(request, &settings, &mut sessions);
                            clients[index].reply(status, &body, id.as_deref());
                            break;
                        }
                        Ok(None) => {}
                        Err(status) => {
                            clients[index].reply(status, &[], None);
                            break;
                        }
                    }
                }
            } else if events & os::POLLHUP != 0 {
                clients[index].closed = true;
            }
        }
        if fds[0].revents & os::POLLIN != 0 {
            for _ in 0..16 {
                let fd = unsafe {
                    accept4(
                        listener.0,
                        core::ptr::null_mut(),
                        core::ptr::null_mut(),
                        SOCK_NONBLOCK | SOCK_CLOEXEC,
                    )
                };
                if fd < 0 {
                    break;
                }
                if clients.len() < MAX_CLIENTS {
                    clients.push(Client::new(fd));
                } else {
                    unsafe {
                        os::close(fd);
                    }
                }
            }
        }
    }
}

fn authority(host: &str, port: u16) -> String {
    authority_for(host, port)
}
fn authority_for(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn bind_listener(host: &str, port: u16, authenticated: bool) -> Result<(Fd, u16, bool), String> {
    let host = CString::new(host).map_err(|_| "invalid HTTP host")?;
    let service = CString::new(port.to_string()).map_err(|_| "invalid HTTP port")?;
    let hints = AddrInfo {
        flags: 0,
        family: 0,
        socktype: SOCK_STREAM,
        protocol: 0,
        addrlen: 0,
        addr: core::ptr::null_mut(),
        canonname: core::ptr::null_mut(),
        next: core::ptr::null_mut(),
    };
    let mut result = core::ptr::null_mut();
    let status = unsafe { getaddrinfo(host.as_ptr(), service.as_ptr(), &hints, &mut result) };
    if status != 0 {
        let message = unsafe { CStr::from_ptr(gai_strerror(status)) }.to_string_lossy();
        return Err(format!("cannot resolve MCP host: {message}"));
    }
    let mut pointer = result;
    let mut selected = None;
    let mut denied = false;
    while !pointer.is_null() {
        let address = unsafe { &*pointer };
        let local = is_loopback(address.family, unsafe {
            core::slice::from_raw_parts(address.addr.cast::<u8>(), address.addrlen as usize)
        });
        if !local && !authenticated {
            denied = true;
            pointer = address.next;
            continue;
        }
        let raw = unsafe {
            socket(
                address.family,
                SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC,
                address.protocol,
            )
        };
        if raw >= 0 {
            let fd = Fd(raw);
            let yes = 1i32;
            unsafe {
                setsockopt(fd.0, 1, 2, (&yes as *const i32).cast(), 4);
            }
            if address.family == 10 {
                unsafe {
                    setsockopt(fd.0, 41, 26, (&yes as *const i32).cast(), 4);
                }
            }
            if unsafe { bind(fd.0, address.addr, address.addrlen) } == 0
                && unsafe { listen(fd.0, 64) } == 0
            {
                let mut storage = [0u64; 16];
                let mut size = core::mem::size_of_val(&storage) as u32;
                if unsafe { getsockname(fd.0, storage.as_mut_ptr().cast(), &mut size) } == 0 {
                    let bytes = unsafe {
                        core::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), size as usize)
                    };
                    let actual = u16::from_be_bytes([bytes[2], bytes[3]]);
                    selected = Some((fd, actual, local));
                    break;
                }
            }
        }
        pointer = address.next;
    }
    unsafe {
        freeaddrinfo(result);
    }
    selected.ok_or_else(|| {
        if denied {
            "non-loopback HTTP listeners require ZTTY_MCP_TOKEN or --token".into()
        } else {
            "cannot bind MCP HTTP listener".into()
        }
    })
}

fn is_loopback(family: i32, address: &[u8]) -> bool {
    match family {
        2 => address.get(4) == Some(&127),
        10 if address.len() >= 24 => address[8..23].iter().all(|&b| b == 0) && address[23] == 1,
        _ => false,
    }
}

struct Request {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}
impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

fn parse_request(bytes: &[u8]) -> Result<Option<Request>, u16> {
    let Some(end) = bytes.windows(4).position(|s| s == b"\r\n\r\n") else {
        return if bytes.len() > MAX_HEADERS {
            Err(431)
        } else {
            Ok(None)
        };
    };
    if end > MAX_HEADERS {
        return Err(431);
    }
    let head = core::str::from_utf8(&bytes[..end]).map_err(|_| 400u16)?;
    let mut lines = head.split("\r\n");
    let mut parts = lines.next().ok_or(400u16)?.split(' ');
    let method = parts.next().ok_or(400u16)?;
    let target = parts.next().ok_or(400u16)?;
    if parts.next() != Some("HTTP/1.1")
        || parts.next().is_some()
        || method.is_empty()
        || target.is_empty()
    {
        return Err(400);
    }
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        let (key, value) = line.split_once(':').ok_or(400u16)?;
        if key.is_empty()
            || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || value.bytes().any(|b| b < 0x20 && b != b'\t')
        {
            return Err(400);
        }
        let key = key.to_ascii_lowercase();
        if headers.iter().any(|(old, _)| old == &key) {
            return Err(400);
        }
        headers.push((key, value.trim().into()));
        if headers.len() > 100 {
            return Err(431);
        }
    }
    let get = |key: &str| {
        headers
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    };
    if get("host").is_none() {
        return Err(400);
    }
    let raw = &bytes[end + 4..];
    let body = if let Some(encoding) = get("transfer-encoding") {
        if get("content-length").is_some() || !encoding.eq_ignore_ascii_case("chunked") {
            return Err(400);
        }
        match chunks(raw)? {
            Some(value) => value,
            None => return Ok(None),
        }
    } else if let Some(length) = get("content-length") {
        if length.is_empty() || !length.bytes().all(|b| b.is_ascii_digit()) {
            return Err(400);
        }
        let length: usize = length.parse().map_err(|_| 413u16)?;
        if length > json::MAX_MESSAGE {
            return Err(413);
        }
        if raw.len() < length {
            return Ok(None);
        }
        raw[..length].to_vec()
    } else if method == "POST" {
        return Err(411);
    } else {
        Vec::new()
    };
    Ok(Some(Request {
        method: method.into(),
        target: target.into(),
        headers,
        body,
    }))
}

fn chunks(bytes: &[u8]) -> Result<Option<Vec<u8>>, u16> {
    let mut offset = 0;
    let mut output = Vec::new();
    loop {
        let Some(line) = bytes[offset..].windows(2).position(|s| s == b"\r\n") else {
            return Ok(None);
        };
        if line > 1024 {
            return Err(400);
        }
        let text = core::str::from_utf8(&bytes[offset..offset + line]).map_err(|_| 400u16)?;
        let size = text.split(';').next().ok_or(400u16)?;
        if size.is_empty() || !size.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(400);
        }
        let size = usize::from_str_radix(size, 16).map_err(|_| 413u16)?;
        offset += line + 2;
        if size == 0 {
            if bytes.get(offset..offset + 2) == Some(b"\r\n") {
                return Ok(Some(output));
            }
            let Some(end) = bytes[offset..].windows(4).position(|s| s == b"\r\n\r\n") else {
                return Ok(None);
            };
            if end > MAX_HEADERS {
                return Err(431);
            }
            return Ok(Some(output));
        }
        if output.len().saturating_add(size) > json::MAX_MESSAGE {
            return Err(413);
        }
        if bytes.len().saturating_sub(offset) < size.saturating_add(2) {
            return Ok(None);
        }
        output.extend_from_slice(&bytes[offset..offset + size]);
        offset += size;
        if bytes.get(offset..offset + 2) != Some(b"\r\n") {
            return Err(400);
        }
        offset += 2;
    }
}

fn contains_type(value: Option<&str>, mime: &str) -> bool {
    value.is_some_and(|value| {
        value.split(',').any(|part| {
            part.split(';')
                .next()
                .is_some_and(|part| part.trim().eq_ignore_ascii_case(mime))
        })
    })
}

fn token_matches(expected: &str, received: &str) -> bool {
    let mismatch = expected
        .bytes()
        .zip(received.bytes())
        .fold(expected.len() ^ received.len(), |sum, (a, b)| {
            sum | usize::from(a ^ b)
        });
    mismatch == 0
}

fn respond(
    request: Request,
    settings: &Settings<'_>,
    sessions: &mut Vec<Session>,
) -> (u16, Vec<u8>, Option<String>) {
    if let Some(token) = settings.token {
        let received = request
            .header("authorization")
            .and_then(|s| s.strip_prefix("Bearer "));
        if received.is_none_or(|value| !token_matches(token, value)) {
            return (401, Vec::new(), None);
        }
    }
    if settings.local
        && request.header("host").is_none_or(|host| {
            !settings
                .authorities
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(host))
        })
    {
        return (403, Vec::new(), None);
    }
    if let Some(origin) = request.header("origin") {
        let accepted = ["http://", "https://"].iter().any(|prefix| {
            origin.strip_prefix(prefix).is_some_and(|authority| {
                settings
                    .authorities
                    .iter()
                    .any(|allowed| allowed.eq_ignore_ascii_case(authority))
            })
        });
        if !accepted {
            return (403, Vec::new(), None);
        }
    }
    if request.target != "/mcp" {
        return (404, Vec::new(), None);
    }
    if request
        .header("mcp-protocol-version")
        .is_some_and(|version| !VERSIONS.contains(&version))
    {
        return (400, Vec::new(), None);
    }
    let existing = request
        .header("mcp-session-id")
        .map(|id| sessions.iter().position(|session| session.id == id));
    if existing == Some(None) {
        return (404, Vec::new(), None);
    }
    let existing = existing.flatten();
    if let Some(index) = existing {
        if request
            .header("mcp-protocol-version")
            .is_some_and(|version| version != sessions[index].rpc.version)
        {
            return (400, Vec::new(), None);
        }
        sessions[index].touched = os::milliseconds();
    }
    if request.method == "GET" {
        return (405, Vec::new(), None);
    }
    if request.method == "DELETE" {
        let Some(index) = existing else {
            return (400, Vec::new(), None);
        };
        sessions.remove(index);
        return (200, Vec::new(), None);
    }
    if request.method != "POST" {
        return (405, Vec::new(), None);
    }
    if !request.header("content-type").is_some_and(|value| {
        value
            .split(';')
            .next()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
    }) {
        return (415, Vec::new(), None);
    }
    if !contains_type(request.header("accept"), "application/json")
        || !contains_type(request.header("accept"), "text/event-stream")
    {
        return (406, Vec::new(), None);
    }
    let message = match json::parse(&request.body) {
        Ok(message) => message,
        Err(message) => {
            return (
                400,
                json::stringify(&error(Value::Null, -32700, message)).into_bytes(),
                None,
            );
        }
    };
    let is_initialize = message.get("method").and_then(Value::as_str) == Some("initialize")
        && message.get("id").is_some();
    let (response, session_id) = if let Some(index) = existing {
        (
            dispatch(&message, &mut sessions[index].rpc, &settings.origin),
            None,
        )
    } else if is_initialize {
        if sessions.len() >= MAX_SESSIONS {
            return (503, Vec::new(), None);
        }
        let mut rpc = RpcSession::new();
        let response = dispatch(&message, &mut rpc, &settings.origin);
        if rpc.initialized {
            let id = match random_id() {
                Ok(id) => id,
                Err(()) => return (503, Vec::new(), None),
            };
            sessions.push(Session {
                id: id.clone(),
                rpc,
                touched: os::milliseconds(),
            });
            (response, Some(id))
        } else {
            (response, None)
        }
    } else {
        return (400, Vec::new(), None);
    };
    match response {
        Some(response) => (200, json::stringify(&response).into_bytes(), session_id),
        None => (202, Vec::new(), None),
    }
}

fn random_id() -> Result<String, ()> {
    let mut random = [0u8; 24];
    let mut done = 0;
    while done < random.len() {
        let count =
            unsafe { getrandom(random[done..].as_mut_ptr().cast(), random.len() - done, 0) };
        if count > 0 {
            done += count as usize;
        } else if count < 0 && os::errno() == os::EINTR {
            continue;
        } else {
            return Err(());
        }
    }
    let mut output = String::with_capacity(random.len() * 2);
    for byte in random {
        const HEX: &[u8] = b"0123456789abcdef";
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 15) as usize] as char);
    }
    Ok(output)
}
