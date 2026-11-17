//! Same-user, single-threaded session broker. No threads or per-window servers.
use crate::{
    json::{self, Value},
    os,
};
use alloc::{ffi::CString, format, string::String, vec::Vec};
use core::ffi::{c_int, c_void};

const LIMIT: usize = 1024 * 1024;
const MAX_PEERS: usize = 128;
const CLOEXEC: i32 = 0o2000000;
const NONBLOCK: i32 = 0o4000;
const NOSIGNAL: i32 = 0x4000;

#[repr(C)]
struct Address {
    family: u16,
    path: [u8; 108],
}
#[repr(C)]
struct Credentials {
    pid: i32,
    uid: u32,
    gid: u32,
}

#[link(name = "c")]
unsafe extern "C" {
    fn socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int;
    fn connect(fd: c_int, address: *const Address, size: u32) -> c_int;
    fn bind(fd: c_int, address: *const c_void, size: u32) -> c_int;
    fn listen(fd: c_int, backlog: c_int) -> c_int;
    fn accept4(fd: c_int, address: *mut c_void, size: *mut u32, flags: c_int) -> c_int;
    fn getsockopt(
        fd: c_int,
        level: c_int,
        option: c_int,
        value: *mut c_void,
        size: *mut u32,
    ) -> c_int;
    fn recv(fd: c_int, bytes: *mut c_void, size: usize, flags: c_int) -> isize;
    fn send(fd: c_int, bytes: *const c_void, size: usize, flags: c_int) -> isize;
    fn setsid() -> c_int;
    fn dup2(old: c_int, new: c_int) -> c_int;
    fn close_range(first: u32, last: u32, flags: c_int) -> c_int;
}

pub struct Fd(pub i32);
impl Drop for Fd {
    fn drop(&mut self) {
        unsafe {
            os::close(self.0);
        }
    }
}

fn address() -> Result<(Address, u32), String> {
    let default = format!("@ztty-{}", unsafe { os::getuid() });
    let name = os::env(c"ZTTY_SOCKET").unwrap_or(default.as_bytes());
    if name.first() != Some(&b'@') || name.len() < 2 || name.len() > 107 || name.contains(&0) {
        return Err("ZTTY_SOCKET must be an abstract Unix address: @name (2..107 bytes)".into());
    }
    let mut address = Address {
        family: 1,
        path: [0; 108],
    };
    address.path[1..name.len()].copy_from_slice(&name[1..]);
    Ok((address, 2 + name.len() as u32))
}

fn same_user(fd: i32) -> bool {
    let mut credentials = Credentials {
        pid: 0,
        uid: u32::MAX,
        gid: u32::MAX,
    };
    let mut size = core::mem::size_of::<Credentials>() as u32;
    unsafe {
        getsockopt(
            fd,
            1,
            17,
            (&mut credentials as *mut Credentials).cast(),
            &mut size,
        ) == 0
            && credentials.uid == os::getuid()
    }
}

fn connection() -> Result<Fd, String> {
    let (address, size) = address()?;
    let fd = Fd(unsafe { socket(1, 1 | CLOEXEC | NONBLOCK, 0) });
    if fd.0 < 0 {
        return Err("cannot create broker connection".into());
    }
    if unsafe { connect(fd.0, &address, size) } < 0 {
        return Err(format!(
            "cannot connect to session broker (OS error {})",
            os::errno()
        ));
    }
    if !same_user(fd.0) {
        return Err("session broker belongs to a different user".into());
    }
    Ok(fd)
}

fn start() -> Result<(), String> {
    let pid = unsafe { os::fork() };
    if pid < 0 {
        return Err("cannot start session broker".into());
    }
    if pid == 0 {
        unsafe {
            if setsid() < 0 {
                os::_exit(1);
            }
            let child = os::fork();
            if child < 0 {
                os::_exit(1);
            }
            if child > 0 {
                os::_exit(0);
            }
            let null = os::open(c"/dev/null".as_ptr(), 2);
            if null < 0 {
                os::_exit(1);
            }
            for fd in 0..3 {
                dup2(null, fd);
            }
            if close_range(3, u32::MAX, 0) < 0 {
                for fd in 3..65536 {
                    os::close(fd);
                }
            }
        }
        let code = if crate::serve().is_ok() { 0 } else { 1 };
        unsafe {
            os::_exit(code);
        }
    }
    loop {
        if unsafe { os::waitpid(pid, core::ptr::null_mut(), 0) } >= 0 || os::errno() != os::EINTR {
            break;
        }
    }
    Ok(())
}

fn ensure() -> Result<Fd, String> {
    if let Ok(fd) = connection() {
        return Ok(fd);
    }
    address()?;
    start()?;
    for _ in 0..250 {
        if let Ok(fd) = connection() {
            return Ok(fd);
        }
        unsafe {
            os::poll(core::ptr::null_mut(), 0, 20);
        }
    }
    connection()
}

fn ready(fd: i32, events: i16, deadline: Option<u64>) -> Result<(), String> {
    loop {
        let timeout = match deadline {
            Some(end) => {
                let now = os::milliseconds();
                if now >= end {
                    return Err("session broker request timed out".into());
                }
                end.saturating_sub(now).min(i32::MAX as u64) as i32
            }
            None => -1,
        };
        let mut item = os::PollFd {
            fd,
            events,
            revents: 0,
        };
        let count = unsafe { os::poll(&mut item, 1, timeout) };
        if count < 0 && os::errno() == os::EINTR {
            continue;
        }
        if count < 0 {
            return Err("session broker poll failed".into());
        }
        if count == 0 {
            return Err("session broker request timed out".into());
        }
        if item.revents & (events | os::POLLHUP) != 0 {
            return Ok(());
        }
        return Err("session broker disconnected".into());
    }
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 15) as usize] as char);
    }
    out
}

pub fn unhex(value: &str) -> Result<CString, String> {
    if value.len() % 2 != 0 {
        return Err("invalid byte string".into());
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
        let digit = |b: u8| -> Option<u8> {
            match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                _ => None,
            }
        };
        bytes.push(
            (digit(pair[0]).ok_or("invalid byte string")? << 4)
                | digit(pair[1]).ok_or("invalid byte string")?,
        );
    }
    CString::new(bytes).map_err(|_| "NUL in process metadata".into())
}

fn caller_metadata(value: &mut Value) -> Result<(), String> {
    if matches!(
        value.get("op").and_then(Value::as_str),
        Some("create" | "attach")
    ) {
        value.set(
            "_env",
            Value::Array(
                os::environment()
                    .iter()
                    .map(|s| Value::from(hex(s.to_bytes())))
                    .collect(),
            ),
        );
        let mut cwd = alloc::vec![0u8; 4096];
        if unsafe { os::getcwd(cwd.as_mut_ptr().cast(), cwd.len()) }.is_null() {
            return Err("cannot read current directory".into());
        }
        let len = cwd
            .iter()
            .position(|&b| b == 0)
            .ok_or("working directory is too long")?;
        value.set("_cwd", Value::from(hex(&cwd[..len])));
    }
    Ok(())
}

/// Send one operation. Command metadata travels only over same-user local IPC.
pub fn request(value: &Value) -> Result<Value, String> {
    let mut value = value.clone();
    caller_metadata(&mut value)?;
    let wait = value.get("op").and_then(Value::as_str) == Some("wait");
    let fd = ensure()?;
    let mut wire = json::stringify(&value).into_bytes();
    if wire.len() >= LIMIT {
        return Err("session request exceeds 1 MiB".into());
    }
    wire.push(b'\n');
    let end = Some(os::milliseconds() + 5000);
    let mut offset = 0;
    while offset < wire.len() {
        ready(fd.0, os::POLLOUT, end)?;
        let n = unsafe {
            send(
                fd.0,
                wire[offset..].as_ptr().cast(),
                wire.len() - offset,
                NOSIGNAL,
            )
        };
        if n > 0 {
            offset += n as usize;
        } else if n < 0 && matches!(os::errno(), 4 | 11) {
            continue;
        } else {
            return Err("session broker send failed".into());
        }
    }
    let mut reply = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        ready(fd.0, os::POLLIN, if wait { None } else { end })?;
        let n = unsafe { recv(fd.0, buffer.as_mut_ptr().cast(), buffer.len(), 0) };
        if n < 0 && matches!(os::errno(), 4 | 11) {
            continue;
        }
        if n <= 0 {
            return Err("session broker disconnected before replying".into());
        }
        if reply.len() + n as usize > LIMIT {
            return Err("session reply exceeds 1 MiB".into());
        }
        reply.extend_from_slice(&buffer[..n as usize]);
        if let Some(newline) = reply.iter().position(|&b| b == b'\n') {
            let envelope = json::parse(&reply[..newline]).map_err(String::from)?;
            if envelope.get("ok").and_then(Value::as_bool) == Some(true) {
                return envelope
                    .get("result")
                    .cloned()
                    .ok_or_else(|| "missing session result".into());
            }
            return Err(envelope
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("session request failed")
                .into());
        }
    }
}

pub enum Reply {
    Value(Value),
    Wait(String),
}

struct Peer {
    fd: Fd,
    input: Vec<u8>,
    output: Vec<u8>,
    sent: usize,
    wait: Option<String>,
    deadline: u64,
    done: bool,
}

impl Peer {
    fn reply(&mut self, value: Result<Value, String>) {
        let envelope = match value {
            Ok(value) => json::object([("ok", Value::Bool(true)), ("result", value)]),
            Err(error) => json::object([("ok", Value::Bool(false)), ("error", Value::from(error))]),
        };
        self.input.clear();
        self.input.shrink_to_fit();
        self.output = json::stringify(&envelope).into_bytes();
        self.output.push(b'\n');
        self.wait = None;
        self.deadline = os::milliseconds() + 5000;
    }
}

pub struct Server {
    pub fd: Fd,
    peers: Vec<Peer>,
}

impl Server {
    pub fn new() -> Result<Self, String> {
        let (address, size) = address()?;
        let fd = Fd(unsafe { socket(1, 1 | CLOEXEC | NONBLOCK, 0) });
        if fd.0 < 0
            || unsafe { bind(fd.0, (&address as *const Address).cast(), size) } < 0
            || unsafe { listen(fd.0, 64) } < 0
        {
            return Err("cannot bind session broker".into());
        }
        Ok(Self {
            fd,
            peers: Vec::new(),
        })
    }

    pub fn descriptors(&self, fds: &mut Vec<os::PollFd>) {
        fds.push(os::PollFd {
            fd: self.fd.0,
            events: os::POLLIN,
            revents: 0,
        });
        for peer in &self.peers {
            fds.push(os::PollFd {
                fd: peer.fd.0,
                events: if peer.output.is_empty() {
                    os::POLLIN
                } else {
                    os::POLLOUT
                },
                revents: 0,
            });
        }
    }

    pub fn has_pending(&self) -> bool {
        self.peers.iter().any(|p| p.wait.is_none())
    }

    pub fn finish(&mut self, name: &str, status: i32) {
        for peer in &mut self.peers {
            if peer.wait.as_deref() == Some(name) {
                peer.reply(Ok(json::object([("exit_code", Value::from(status))])));
            }
        }
    }

    pub fn pump(&mut self, mut handle: impl FnMut(&Value) -> Result<Reply, String>) {
        for _ in 0..64 {
            let fd = unsafe {
                accept4(
                    self.fd.0,
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                    CLOEXEC | NONBLOCK,
                )
            };
            if fd < 0 {
                break;
            }
            let fd = Fd(fd);
            if self.peers.len() >= MAX_PEERS || !same_user(fd.0) {
                continue;
            }
            self.peers.push(Peer {
                fd,
                input: Vec::new(),
                output: Vec::new(),
                sent: 0,
                wait: None,
                deadline: os::milliseconds() + 5000,
                done: false,
            });
        }
        let mut total_input: usize = self.peers.iter().map(|p| p.input.len()).sum();
        let mut buffer = [0u8; 8192];
        for peer in &mut self.peers {
            if peer.wait.is_none() && os::milliseconds() > peer.deadline {
                peer.done = true;
                continue;
            }
            if !peer.output.is_empty() {
                let n = unsafe {
                    send(
                        peer.fd.0,
                        peer.output[peer.sent..].as_ptr().cast(),
                        peer.output.len() - peer.sent,
                        NOSIGNAL,
                    )
                };
                if n > 0 {
                    peer.sent += n as usize;
                    peer.done = peer.sent == peer.output.len();
                } else if n == 0 || !matches!(os::errno(), 4 | 11) {
                    peer.done = true;
                }
                continue;
            }
            for _ in 0..32 {
                let n = unsafe { recv(peer.fd.0, buffer.as_mut_ptr().cast(), buffer.len(), 0) };
                if n < 0 && os::errno() == os::EINTR {
                    continue;
                }
                if n < 0 && os::errno() == 11 {
                    break;
                }
                if n <= 0 {
                    peer.done = true;
                    break;
                }
                if peer.wait.is_some() {
                    peer.done = true;
                    break;
                }
                total_input += n as usize;
                if peer.input.len() + n as usize >= LIMIT || total_input > 2 * LIMIT {
                    peer.reply(Err("session input limit exceeded".into()));
                    break;
                }
                peer.input.extend_from_slice(&buffer[..n as usize]);
                if let Some(newline) = peer.input.iter().position(|&b| b == b'\n') {
                    let result = json::parse(&peer.input[..newline])
                        .map_err(String::from)
                        .and_then(|v| handle(&v));
                    total_input = total_input.saturating_sub(peer.input.len());
                    match result {
                        Ok(Reply::Wait(name)) => {
                            peer.wait = Some(name);
                            peer.input = Vec::new();
                        }
                        Ok(Reply::Value(value)) => peer.reply(Ok(value)),
                        Err(error) => peer.reply(Err(error)),
                    }
                    break;
                }
            }
        }
        self.peers.retain(|p| !p.done);
    }
}
