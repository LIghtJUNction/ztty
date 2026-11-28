//! Same-user, single-threaded session broker. No threads or per-window servers.
mod encoding;
mod reload;
use crate::{
    json::{self, Value},
    os,
};
use alloc::{ffi::CString, format, string::String, vec::Vec};
use core::ffi::{CStr, c_char, c_int, c_void};
use core::sync::atomic::{AtomicU64, Ordering};
pub use encoding::{hex, unhex, unhex_bytes};

const LIMIT: usize = 1024 * 1024;
const MAX_PEERS: usize = 192;
const MAX_WAITERS: usize = 128;
pub(crate) const MAX_INPUT_DRAIN_MS: u64 = 5000;
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
    fn setenv(name: *const c_char, value: *const c_char, overwrite: c_int) -> c_int;
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
    named_address(name)
}

fn named_address(name: &[u8]) -> Result<(Address, u32), String> {
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

fn credentials(fd: i32) -> Option<Credentials> {
    let mut credentials = Credentials {
        pid: 0,
        uid: u32::MAX,
        gid: u32::MAX,
    };
    let mut size = core::mem::size_of::<Credentials>() as u32;
    let valid = unsafe {
        getsockopt(
            fd,
            1,
            17,
            (&mut credentials as *mut Credentials).cast(),
            &mut size,
        )
    } == 0;
    (valid && size as usize == core::mem::size_of::<Credentials>() && credentials.pid > 0)
        .then_some(credentials)
}

fn same_user(fd: i32) -> bool {
    credentials(fd).is_some_and(|peer| peer.uid == unsafe { os::getuid() })
}

enum Connection {
    Ready(Fd),
    Connecting(Fd),
    Unavailable,
    Busy,
}

fn connection() -> Result<Connection, String> {
    let (address, size) = address()?;
    connection_at(&address, size)
}

fn connection_at(address: &Address, size: u32) -> Result<Connection, String> {
    let fd = Fd(unsafe { socket(1, 1 | CLOEXEC | NONBLOCK, 0) });
    if fd.0 < 0 {
        return Err("cannot create broker connection".into());
    }
    if unsafe { connect(fd.0, address, size) } < 0 {
        return match os::errno() {
            2 | 111 => Ok(Connection::Unavailable),
            4 | 11 => Ok(Connection::Busy),
            114 | 115 => Ok(Connection::Connecting(fd)),
            error => Err(format!(
                "cannot connect to session broker (OS error {error})"
            )),
        };
    }
    if !same_user(fd.0) {
        return Err("session broker belongs to a different user".into());
    }
    Ok(Connection::Ready(fd))
}

// A content tag chooses a broker namespace; it is not an authentication token.
// Hash only during a standalone GUI launch, never in the broker's event loop.
#[derive(Clone, Copy, PartialEq, Eq)]
struct ImageTag {
    hash: u64,
    bytes: u64,
}

fn image_tag(path: &CStr) -> Result<ImageTag, String> {
    let fd = Fd(unsafe { os::open(path.as_ptr(), CLOEXEC) });
    if fd.0 < 0 {
        return Err(format!(
            "cannot inspect terminal executable (OS error {})",
            os::errno()
        ));
    }
    let mut tag = ImageTag {
        hash: 0xcbf29ce484222325,
        bytes: 0,
    };
    let mut buffer = [0u8; 16384];
    loop {
        let count = unsafe { os::read(fd.0, buffer.as_mut_ptr().cast(), buffer.len()) };
        if count == 0 {
            return Ok(tag);
        }
        if count < 0 {
            if os::errno() == os::EINTR {
                continue;
            }
            return Err(format!(
                "cannot read terminal executable (OS error {})",
                os::errno()
            ));
        }
        tag.bytes += count as u64;
        for &byte in &buffer[..count as usize] {
            tag.hash = (tag.hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
}

fn peer_image(fd: i32) -> Result<ImageTag, String> {
    let peer = credentials(fd)
        .filter(|peer| peer.uid == unsafe { os::getuid() })
        .ok_or("cannot identify same-user session broker")?;
    let path = CString::new(format!("/proc/{}/exe", peer.pid))
        .map_err(|_| "invalid broker executable path")?;
    image_tag(&path)
}

/// Inspect the peer without sending a request, so a legacy broker receives no
/// create or mutation before its executable has been checked.
fn probe_image(name: &[u8]) -> Result<Option<ImageTag>, String> {
    let (address, size) = named_address(name)?;
    let deadline = os::milliseconds().saturating_add(5000);
    loop {
        match connection_at(&address, size)? {
            Connection::Ready(fd) => return peer_image(fd.0).map(Some),
            Connection::Unavailable => return Ok(None),
            Connection::Connecting(fd) => {
                let mut poll = os::PollFd {
                    fd: fd.0,
                    events: os::POLLOUT,
                    revents: 0,
                };
                let timeout = deadline.saturating_sub(os::milliseconds()) as i32;
                let ready = unsafe { os::poll(&mut poll, 1, timeout) };
                if ready > 0 {
                    return peer_image(fd.0).map(Some);
                }
                if ready < 0 && os::errno() != os::EINTR {
                    return Err("cannot poll session broker identity".into());
                }
            }
            Connection::Busy => {
                unsafe { os::poll(core::ptr::null_mut(), 0, 20) };
            }
        }
        if os::milliseconds() >= deadline {
            return Err("session broker identity check timed out".into());
        }
    }
}

fn use_gui_socket(name: &str) -> Result<(), String> {
    let name = CString::new(name).map_err(|_| "invalid GUI broker address")?;
    // This is called once, before Xlib/PTY setup, and only adds an absent entry.
    // Existing explicit addresses and inherited session routing remain intact.
    if unsafe { setenv(c"ZTTY_SOCKET".as_ptr(), name.as_ptr(), 0) } != 0 {
        return Err("cannot select the current terminal broker".into());
    }
    Ok(())
}

/// A new independent window uses the running build when the default broker is
/// from an earlier executable. Existing targeted operations keep their socket.
pub fn select_gui_broker() -> Result<(), String> {
    if os::env(c"ZTTY_SOCKET").is_some() {
        return Ok(());
    }
    let current = image_tag(c"/proc/self/exe")?;
    let uid = unsafe { os::getuid() };
    let upgraded = format!(
        "@ztty-{uid}-image-{:016x}-{:x}",
        current.hash, current.bytes
    );
    if let Some(peer) = probe_image(upgraded.as_bytes())? {
        if peer != current {
            return Err("current-build broker address belongs to a different executable".into());
        }
        return use_gui_socket(&upgraded);
    }
    let default = format!("@ztty-{uid}");
    if probe_image(default.as_bytes())?.is_some_and(|peer| peer != current) {
        use_gui_socket(&upgraded)?;
    }
    Ok(())
}

fn start() -> Result<(), String> {
    // Several MCP calls can observe the same cold socket before the first
    // child has bound it. Share that launch attempt within this process;
    // individual requests keep retrying connect without forking again.
    static NEXT_START: AtomicU64 = AtomicU64::new(0);
    let now = os::milliseconds();
    let previous = NEXT_START.load(Ordering::Relaxed);
    let until = now.saturating_add(250);
    if now < previous
        || NEXT_START
            .compare_exchange(previous, until, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
    {
        return Ok(());
    }
    let pid = unsafe { os::fork() };
    if pid < 0 {
        let _ = NEXT_START.compare_exchange(until, now, Ordering::Relaxed, Ordering::Relaxed);
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum RequestPhase {
    Connecting,
    Sending,
    Receiving,
    Finished,
}

/// One nonblocking operation over same-user IPC. Dropping a request closes its
/// connection; a request that has sent bytes is never reconnected or replayed.
pub struct PendingRequest {
    fd: Option<Fd>,
    phase: RequestPhase,
    wire: Vec<u8>,
    sent: usize,
    reply: Vec<u8>,
    deadline: Option<u64>,
    retry_at: Option<u64>,
    reply_timeout: Option<u64>,
    started: bool,
}

impl PendingRequest {
    pub fn new(value: &Value) -> Result<Self, String> {
        let mut value = value.clone();
        caller_metadata(&mut value)?;
        let operation = value.get("op").and_then(Value::as_str);
        let reply_timeout = if operation == Some("wait") {
            None
        } else if operation == Some("reload") {
            // The pinned replacement gets a bounded 10 s validation window;
            // reserve time for complete native view/layout reconstruction too.
            Some(60_000)
        } else {
            let wait = if operation == Some("cli-send") {
                MAX_INPUT_DRAIN_MS
            } else if operation == Some("write") {
                input_drain_timeout(&value)?.unwrap_or(0)
            } else if matches!(operation, Some("read" | "watch")) {
                match value.get("wait_ms") {
                    None => 0,
                    Some(value) => value
                        .as_u64()
                        .filter(|&wait| wait <= 30000)
                        .ok_or("wait_ms must be 0..30000")?,
                }
            } else {
                0
            };
            Some(wait + 5000)
        };
        let mut wire = json::stringify(&value).into_bytes();
        if wire.len() >= LIMIT {
            return Err("session request exceeds 1 MiB".into());
        }
        wire.push(b'\n');
        let mut pending = Self {
            fd: None,
            phase: RequestPhase::Sending,
            wire,
            sent: 0,
            reply: Vec::new(),
            deadline: Some(os::milliseconds().saturating_add(5000)),
            retry_at: None,
            reply_timeout,
            started: false,
        };
        pending.connect_once()?;
        Ok(pending)
    }

    fn connect_once(&mut self) -> Result<(), String> {
        match connection()? {
            Connection::Ready(fd) => {
                self.fd = Some(fd);
                self.phase = RequestPhase::Sending;
                self.retry_at = None;
            }
            Connection::Connecting(fd) => {
                self.fd = Some(fd);
                self.phase = RequestPhase::Connecting;
                self.retry_at = None;
            }
            unavailable => {
                // A full listener backlog is not a missing broker. Only a
                // refused/missing endpoint may start one, once per request.
                if matches!(unavailable, Connection::Unavailable) && !self.started {
                    start()?;
                    self.started = true;
                }
                self.retry_at = Some(os::milliseconds().saturating_add(20));
            }
        }
        Ok(())
    }

    pub fn poll_fd(&self) -> Option<os::PollFd> {
        self.fd.as_ref().map(|fd| os::PollFd {
            fd: fd.0,
            events: if self.phase == RequestPhase::Receiving {
                os::POLLIN
            } else {
                os::POLLOUT
            },
            revents: 0,
        })
    }

    pub fn next_deadline(&self) -> Option<u64> {
        match (self.deadline, self.retry_at) {
            (Some(deadline), Some(retry)) => Some(deadline.min(retry)),
            (deadline, retry) => deadline.or(retry),
        }
    }

    /// Retained allocation, including already-sent bytes until the request has
    /// been sent completely. Transports can use this for aggregate buffer caps.
    pub fn buffered_bytes(&self) -> usize {
        self.wire.capacity().saturating_add(self.reply.capacity())
    }

    fn finish(&mut self, value: Result<Value, String>) -> Option<Result<Value, String>> {
        self.fd = None;
        self.phase = RequestPhase::Finished;
        self.deadline = None;
        self.retry_at = None;
        self.wire = Vec::new();
        self.reply = Vec::new();
        Some(value)
    }

    /// Advance until EAGAIN or a bounded I/O budget is exhausted. Completion is
    /// yielded once; callers then remove this request from their pending set.
    pub fn advance(&mut self) -> Option<Result<Value, String>> {
        if self.phase == RequestPhase::Finished {
            return None;
        }
        let now = os::milliseconds();
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            return self.finish(Err("session broker request timed out".into()));
        }
        if self.fd.is_none() {
            if self.retry_at.is_some_and(|retry| now < retry) {
                return None;
            }
            if let Err(error) = self.connect_once() {
                return self.finish(Err(error));
            }
        }
        let fd = self.fd.as_ref()?.0;
        if self.phase == RequestPhase::Connecting {
            let mut item = os::PollFd {
                fd,
                events: os::POLLOUT,
                revents: 0,
            };
            let count = unsafe { os::poll(&mut item, 1, 0) };
            if count == 0 || (count < 0 && os::errno() == os::EINTR) {
                return None;
            }
            if count < 0 {
                return self.finish(Err("session broker poll failed".into()));
            }
            let mut error = 0i32;
            let mut size = core::mem::size_of_val(&error) as u32;
            if unsafe { getsockopt(fd, 1, 4, (&mut error as *mut i32).cast(), &mut size) } != 0 {
                return self.finish(Err("cannot inspect broker connection".into()));
            }
            match error {
                0 => {}
                4 | 11 | 114 | 115 => return None,
                2 | 111 => {
                    self.fd = None;
                    self.phase = RequestPhase::Sending;
                    self.retry_at = Some(os::milliseconds().saturating_add(20));
                    return None;
                }
                error => {
                    return self.finish(Err(format!(
                        "cannot connect to session broker (OS error {error})"
                    )));
                }
            }
            if !same_user(fd) {
                return self.finish(Err("session broker belongs to a different user".into()));
            }
            self.phase = RequestPhase::Sending;
        }
        if self.phase == RequestPhase::Sending {
            for _ in 0..32 {
                if self.sent == self.wire.len() {
                    break;
                }
                let end = self.sent.saturating_add(8192).min(self.wire.len());
                let bytes = &self.wire[self.sent..end];
                let count = unsafe { send(fd, bytes.as_ptr().cast(), bytes.len(), NOSIGNAL) };
                if count > 0 {
                    self.sent += count as usize;
                } else if count < 0 && os::errno() == os::EINTR {
                    continue;
                } else if count < 0 && os::errno() == 11 {
                    return None;
                } else {
                    return self.finish(Err("session broker send failed".into()));
                }
            }
            if self.sent < self.wire.len() {
                return None;
            }
            self.wire = Vec::new();
            self.phase = RequestPhase::Receiving;
            self.deadline = self
                .reply_timeout
                .map(|timeout| os::milliseconds().saturating_add(timeout));
        }
        let mut buffer = [0u8; 8192];
        for _ in 0..32 {
            let count = unsafe { recv(fd, buffer.as_mut_ptr().cast(), buffer.len(), 0) };
            if count < 0 && os::errno() == os::EINTR {
                continue;
            }
            if count < 0 && os::errno() == 11 {
                return None;
            }
            if count <= 0 {
                return self.finish(Err("session broker disconnected before replying".into()));
            }
            if self.reply.len() + count as usize > LIMIT {
                return self.finish(Err("session reply exceeds 1 MiB".into()));
            }
            let previous = self.reply.len();
            self.reply.extend_from_slice(&buffer[..count as usize]);
            if let Some(newline) = buffer[..count as usize].iter().position(|&b| b == b'\n') {
                let result = json::parse(&self.reply[..previous + newline])
                    .map_err(String::from)
                    .and_then(|envelope| {
                        if envelope.get("ok").and_then(Value::as_bool) == Some(true) {
                            envelope
                                .get("result")
                                .cloned()
                                .ok_or_else(|| "missing session result".into())
                        } else {
                            Err(envelope
                                .get("error")
                                .and_then(Value::as_str)
                                .unwrap_or("session request failed")
                                .into())
                        }
                    });
                return self.finish(result);
            }
        }
        None
    }
}

/// Send one operation synchronously for CLI callers, using the same bounded
/// transport as the asynchronous MCP frontends.
pub fn request(value: &Value) -> Result<Value, String> {
    let mut pending = PendingRequest::new(value)?;
    loop {
        if let Some(result) = pending.advance() {
            return result;
        }
        let timeout = pending.next_deadline().map_or(-1, |deadline| {
            deadline
                .saturating_sub(os::milliseconds())
                .min(i32::MAX as u64) as i32
        });
        let count = if let Some(mut fd) = pending.poll_fd() {
            unsafe { os::poll(&mut fd, 1, timeout) }
        } else {
            unsafe { os::poll(core::ptr::null_mut(), 0, timeout) }
        };
        if count < 0 && os::errno() != os::EINTR {
            return Err("session broker poll failed".into());
        }
    }
}

pub enum Reply {
    Value(Value),
    Wait(String),
    ReadWait(ReadWait),
}

/// Mux writes can wait for PTY acceptance without changing ordinary MCP write
/// acknowledgements. Validate the bound before any bytes enter the input queue.
pub(crate) fn input_drain_timeout(value: &Value) -> Result<Option<u64>, String> {
    value
        .get("_drain_ms")
        .map(|value| {
            value
                .as_u64()
                .filter(|&wait| (1..=MAX_INPUT_DRAIN_MS).contains(&wait))
                .ok_or_else(|| String::from("_drain_ms must be 1..5000"))
        })
        .transpose()
}

/// A lease held by the broker only while draining one headless CLI write.
pub const CLI_SENDER: &str = "cli:send";

/// Input must reach the PTY under the same ownership epoch that enqueued it.
/// A takeover clears pending input and must never look like a successful drain.
pub struct InputDrain {
    pub actor: String,
    pub control_epoch: u64,
    pub written: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReadUntil {
    Output,
    Eof,
    Exit,
}

impl ReadUntil {
    pub fn parse(value: Option<&Value>) -> Result<Option<Self>, &'static str> {
        match value {
            None | Some(Value::Null) => Ok(None),
            Some(v) => match v.as_str() {
                Some("output") => Ok(Some(Self::Output)),
                Some("eof") => Ok(Some(Self::Eof)),
                Some("exit") => Ok(Some(Self::Exit)),
                _ => Err("invalid output wait condition"),
            },
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Output => "output",
            Self::Eof => "eof",
            Self::Exit => "exit",
        }
    }
}

/// A deferred output read, frame watch or input drain refers to one PTY
/// incarnation, even when the caller used a short name without an identity.
pub struct ReadWait {
    pub until: Option<ReadUntil>,
    pub session_id: u64,
    pub cursor: u64,
    pub max_bytes: Option<u64>,
    pub deadline: u64,
    pub watch_revision: Option<(u64, u64)>,
    pub input_drain: Option<InputDrain>,
}

struct Peer {
    fd: Fd,
    input: Vec<u8>,
    output: Vec<u8>,
    sent: usize,
    wait: Option<String>,
    read_wait: Option<ReadWait>,
    deadline: u64,
    done: bool,
}

impl Peer {
    fn waiting(&self) -> bool {
        self.wait.is_some() || self.read_wait.is_some()
    }

    fn reply(&mut self, value: Result<Value, String>) {
        let envelope = match value {
            Ok(value) => json::object([("ok", Value::Bool(true)), ("result", value)]),
            Err(error) => json::object([("ok", Value::Bool(false)), ("error", Value::from(error))]),
        };
        self.input.clear();
        self.input.shrink_to_fit();
        self.output = json::stringify(&envelope).into_bytes();
        self.output.push(b'\n');
        self.sent = 0;
        self.wait = None;
        self.read_wait = None;
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

    pub fn next_deadline(&self) -> Option<u64> {
        self.peers
            .iter()
            .filter_map(|peer| {
                if let Some(wait) = &peer.read_wait {
                    Some(wait.deadline)
                } else if peer.wait.is_none() {
                    Some(peer.deadline)
                } else {
                    None
                }
            })
            .min()
    }

    /// Run after PTY reads and management operations. Only a ready read builds
    /// its screen snapshot; parked peers retain a bounded wait condition.
    pub fn wake_reads(&mut self, mut read: impl FnMut(&ReadWait) -> Option<Result<Value, String>>) {
        for peer in &mut self.peers {
            if let Some(wait) = &peer.read_wait
                && let Some(result) = read(wait)
            {
                peer.reply(result);
            }
        }
    }

    pub fn has_cli_drain(&self, session_id: u64, epoch: u64) -> bool {
        self.peers
            .iter()
            .filter_map(|peer| peer.read_wait.as_ref())
            .any(|wait| {
                wait.session_id == session_id
                    && wait.input_drain.as_ref().is_some_and(|drain| {
                        drain.actor == CLI_SENDER && drain.control_epoch == epoch
                    })
            })
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
                read_wait: None,
                deadline: os::milliseconds() + 5000,
                done: false,
            });
        }
        let mut waiting = self.peers.iter().filter(|p| p.waiting()).count();
        let mut total_input: usize = self.peers.iter().map(|p| p.input.len()).sum();
        let mut buffer = [0u8; 8192];
        for peer in &mut self.peers {
            if !peer.waiting() && os::milliseconds() >= peer.deadline {
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
                    if peer.waiting() {
                        waiting -= 1;
                    }
                    peer.done = true;
                    break;
                }
                if peer.waiting() {
                    waiting -= 1;
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
                        .and_then(|mut v| {
                            if v.get("op").and_then(Value::as_str) == Some("cli-send")
                                && waiting >= MAX_WAITERS
                            {
                                return Err(
                                    "session waiter limit reached before sending input".into()
                                );
                            }
                            if v.get("op").and_then(Value::as_str) == Some("reload") {
                                let pid = credentials(peer.fd.0)
                                    .ok_or("cannot verify reload caller")?
                                    .pid;
                                v.set("_reload_peer_pid", Value::from(pid));
                            }
                            handle(&v)
                        });
                    total_input = total_input.saturating_sub(peer.input.len());
                    match result {
                        Ok(Reply::Wait(name)) => {
                            if waiting >= MAX_WAITERS && name != crate::runtime_reload::WAIT {
                                peer.reply(Err("session waiter limit reached (128); management connections remain available".into()));
                            } else {
                                waiting += 1;
                                peer.wait = Some(name);
                                peer.input = Vec::new();
                            }
                        }
                        Ok(Reply::ReadWait(read)) => {
                            if waiting >= MAX_WAITERS {
                                peer.reply(Err("session waiter limit reached (128); management connections remain available".into()));
                            } else {
                                waiting += 1;
                                peer.read_wait = Some(read);
                                peer.input = Vec::new();
                            }
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
