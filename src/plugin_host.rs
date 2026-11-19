//! Linux text-extension workers. Native plugins are trusted code, not a sandbox.
//! The broker never loads their images or waits synchronously for their calls.
use alloc::{ffi::CString, format, string::String, vec, vec::Vec};
use core::ffi::{CStr, c_char, c_int, c_long, c_ulong, c_void};
use ztty_plugin_api::{MAX_TEXT_BYTES, TextActionApi};

use crate::os;

const MAGIC: &[u8; 4] = b"ZTP1";
const RESPONSE_HEADER: usize = 12;
const TIMEOUT_MS: u64 = 2000;
const SIGKILL: c_int = 9;
const EAGAIN: c_int = 11;
const O_CLOEXEC: c_int = 0x80000;
const O_NONBLOCK: c_int = 0x800;
const MSG_NOSIGNAL: c_int = 0x4000;

#[repr(C)]
struct RLimit {
    current: c_ulong,
    maximum: c_ulong,
}

// Linux siginfo_t is 128 bytes. Only its first, architecture-independent
// si_signo field is read; reserve an 8-aligned opaque area for the other fields.
#[repr(C, align(8))]
struct ExitInfo {
    signal: c_int,
    _rest: [c_int; 31],
}

unsafe extern "C" {
    fn socketpair(domain: c_int, kind: c_int, protocol: c_int, fds: *mut c_int) -> c_int;
    fn send(fd: c_int, bytes: *const c_void, count: usize, flags: c_int) -> isize;
    fn shutdown(fd: c_int, how: c_int) -> c_int;
    fn dup2(old: c_int, new: c_int) -> c_int;
    fn setpgid(pid: c_int, group: c_int) -> c_int;
    fn getppid() -> c_int;
    fn execve(path: *const c_char, args: *const *const c_char, env: *const *const c_char) -> c_int;
    fn syscall(number: c_long, ...) -> c_long;
    fn setrlimit(resource: c_int, limit: *const RLimit) -> c_int;
    fn prctl(option: c_int, ...) -> c_int;
    fn alarm(seconds: u32) -> u32;
    fn signal(number: c_int, handler: usize) -> usize;
    fn waitid(kind: c_int, id: u32, info: *mut ExitInfo, options: c_int) -> c_int;
}

struct Fd(c_int);

impl Fd {
    fn close(&mut self) {
        if self.0 >= 0 {
            // SAFETY: this object owns the descriptor and invalidates it once.
            unsafe { os::close(self.0) };
            self.0 = -1;
        }
    }

    fn above_stdio(mut self) -> Result<Self, String> {
        if self.0 <= 2 {
            // F_DUPFD_CLOEXEC prevents dup2 in the child from clobbering another
            // prepared descriptor even if the launching process closed stdio.
            let next = unsafe { os::fcntl(self.0, 1030, 3) };
            if next < 0 {
                return Err("cannot prepare extension descriptor".into());
            }
            self.close();
            self.0 = next;
        }
        Ok(self)
    }
}

impl Drop for Fd {
    fn drop(&mut self) {
        self.close();
    }
}

#[derive(Clone, Copy)]
pub enum Mode {
    Check,
    Run,
}

impl Mode {
    fn argument(self) -> &'static CStr {
        match self {
            Self::Check => c"check",
            Self::Run => c"run",
        }
    }
}

/// Keep a job in its owner until `step` returns a result, including after cancel.
/// This lets closed windows retire their workers without blocking the GUI.
#[must_use]
pub struct Job {
    pid: c_int,
    channel: Fd,
    input: Vec<u8>,
    written: usize,
    output: Vec<u8>,
    input_closed: bool,
    eof: bool,
    deadline: u64,
    failure: Option<String>,
}

impl Job {
    pub fn start(path: &str, mode: Mode, input: &str) -> Result<Self, String> {
        if input.len() > MAX_TEXT_BYTES {
            return Err("extension input exceeds 1 MiB".into());
        }
        if !path.starts_with('/') || path.len() > 4096 || path.chars().any(char::is_control) {
            return Err("extension needs an absolute UTF-8 library path".into());
        }
        let path = CString::new(path).map_err(|_| "invalid extension path")?;
        let parent = CString::new(format!("{}", unsafe { os::getpid() })).unwrap();
        let environment = os::environment();
        let env: Vec<*const c_char> = environment
            .iter()
            .map(|entry| entry.as_ptr())
            .chain(core::iter::once(core::ptr::null()))
            .collect();
        let args = [
            c"ztty".as_ptr(),
            c"__plugin-worker".as_ptr(),
            parent.as_ptr(),
            mode.argument().as_ptr(),
            path.as_ptr(),
            core::ptr::null(),
        ];
        let mut sockets = [-1; 2];
        if unsafe { socketpair(1, 1 | O_CLOEXEC, 0, sockets.as_mut_ptr()) } != 0 {
            return Err("cannot create extension channel".into());
        }
        // Own both ends before any fallible preparation, avoiding partial leaks.
        let (host, child) = (Fd(sockets[0]), Fd(sockets[1]));
        let host = host.above_stdio()?;
        let child = child.above_stdio()?;
        let null = unsafe { os::open(c"/dev/null".as_ptr(), 2 | O_CLOEXEC) };
        if null < 0 {
            return Err("cannot open extension error sink".into());
        }
        let null = Fd(null).above_stdio()?;
        let flags = unsafe { os::fcntl(host.0, 3) };
        if flags < 0 || unsafe { os::fcntl(host.0, 4, flags | O_NONBLOCK) } < 0 {
            return Err("cannot make extension channel nonblocking".into());
        }
        let mut request = Vec::with_capacity(8 + input.len());
        request.extend_from_slice(MAGIC);
        request.extend_from_slice(&(input.len() as u32).to_le_bytes());
        request.extend_from_slice(input.as_bytes());
        // ztty reaps its own children. An explicitly inherited SIGCHLD ignore
        // disposition would auto-reap this child and permit numeric PID reuse
        // before cancellation; restore the default before acquiring ownership.
        if unsafe { signal(17, 0) } == usize::MAX {
            return Err("cannot establish extension child ownership".into());
        }
        // Every allocation and environment lookup precedes fork. The child
        // only adjusts its descriptors/process group and immediately execs.
        let pid = unsafe { os::fork() };
        if pid < 0 {
            return Err("cannot start extension worker".into());
        }
        if pid == 0 {
            unsafe {
                if setpgid(0, 0) != 0
                    || dup2(child.0, 0) < 0
                    || dup2(child.0, 1) < 0
                    || dup2(null.0, 2) < 0
                    // Linux close_range via syscall avoids a new glibc symbol
                    // requirement. Fail closed on kernels before Linux 5.9.
                    || syscall(436, 3u32, u32::MAX, 0u32) != 0
                {
                    os::_exit(126);
                }
                execve(c"/proc/self/exe".as_ptr(), args.as_ptr(), env.as_ptr());
                os::_exit(127);
            }
        }
        // Either parent or child may establish the group first. The child also
        // does it before exec; failures here can only be that harmless race.
        unsafe { setpgid(pid, pid) };
        Ok(Self {
            pid,
            channel: host,
            input: request,
            written: 0,
            output: Vec::new(),
            input_closed: false,
            eof: false,
            deadline: os::milliseconds() + TIMEOUT_MS,
            failure: None,
        })
    }

    pub fn descriptor(&self) -> Option<os::PollFd> {
        (self.channel.0 >= 0).then_some(os::PollFd {
            fd: self.channel.0,
            events: os::POLLIN | if self.input_closed { 0 } else { os::POLLOUT },
            revents: 0,
        })
    }

    pub fn next_deadline(&self, now: u64) -> u64 {
        if self.failure.is_some() || self.eof {
            now + 10 // Nonblocking waitpid retries after a channel is closed.
        } else {
            self.deadline
        }
    }

    pub fn cancel(&mut self, reason: &str) {
        if self.failure.is_none() {
            self.failure = Some(reason.into());
            self.channel.close();
            self.input.clear();
            self.output.clear();
            self.stop();
        }
    }

    fn stop(&self) {
        if self.pid > 0 {
            // The direct child has not been reaped, so its PID cannot be reused.
            // Its group was created for this job alone, never a user PTY group.
            unsafe {
                os::kill(-self.pid, SIGKILL);
                os::kill(self.pid, SIGKILL);
            }
        }
    }

    pub fn step(&mut self, now: u64) -> Option<Result<String, String>> {
        if self.pid <= 0 {
            return None;
        }
        if now >= self.deadline && self.failure.is_none() {
            self.cancel("extension timed out after 2 seconds");
        }
        if self.failure.is_none() && !self.eof {
            // Bound work per GUI iteration, even when several windows run jobs.
            for _ in 0..4 {
                if self.written == self.input.len() {
                    if !self.input_closed {
                        unsafe { shutdown(self.channel.0, 1) };
                        self.input_closed = true;
                    }
                    break;
                }
                let bytes = &self.input[self.written..];
                let count = unsafe {
                    send(
                        self.channel.0,
                        bytes.as_ptr().cast(),
                        bytes.len().min(4096),
                        MSG_NOSIGNAL,
                    )
                };
                if count > 0 {
                    self.written += count as usize;
                } else if count < 0 && os::errno() == os::EINTR {
                    continue;
                } else if count < 0 && os::errno() == EAGAIN {
                    break;
                } else {
                    self.cancel("extension worker stopped while receiving input");
                    break;
                }
            }
            let mut bytes = [0u8; 4096];
            for _ in 0..4 {
                if self.failure.is_some() {
                    break;
                }
                let count =
                    unsafe { os::read(self.channel.0, bytes.as_mut_ptr().cast(), bytes.len()) };
                if count == 0 {
                    self.eof = true;
                    self.channel.close();
                    break;
                } else if count < 0 && os::errno() == os::EINTR {
                    continue;
                } else if count < 0 && os::errno() == EAGAIN {
                    break;
                } else if count < 0 {
                    self.cancel("cannot read extension result");
                    break;
                }
                if self.output.len() + count as usize > MAX_TEXT_BYTES + RESPONSE_HEADER {
                    self.cancel("extension output exceeds 1 MiB");
                    break;
                }
                self.output.extend_from_slice(&bytes[..count as usize]);
                if self.output.len() >= RESPONSE_HEADER
                    && (self.output[..4] != *MAGIC
                        || u32::from_le_bytes(self.output[8..12].try_into().unwrap()) as usize
                            > MAX_TEXT_BYTES)
                {
                    self.cancel("invalid extension response");
                    break;
                }
            }
        }
        // Do not reap a child while a descendant may still hold its channel:
        // timeout/cancel must be able to kill that original, unreused group.
        if self.failure.is_none() && !self.eof {
            return None;
        }
        // EOF alone does not prove that the whole owned group has exited:
        // a faulty plugin can fork a descendant that closes the channel. Keep
        // the exited leader unreaped (WNOWAIT) until the group is stopped, so
        // its numeric PID/PGID cannot be reused for an unrelated process.
        let mut info = ExitInfo {
            signal: 0,
            _rest: [0; 31],
        };
        let observed = unsafe { waitid(1, self.pid as u32, &mut info, 4 | 1 | 0x0100_0000) };
        if observed == 0 && info.signal == 0 || observed < 0 && os::errno() == os::EINTR {
            return None;
        }
        if observed < 0 {
            if os::errno() == 10 {
                // ECHILD: identity ownership was lost; never signal it.
                self.pid = 0;
                return Some(Err("extension worker ownership was lost".into()));
            }
            self.cancel("cannot observe extension worker exit");
        } else {
            self.stop();
        }
        let mut status = 0;
        let waited = unsafe { os::waitpid(self.pid, &mut status, 1) };
        if waited == 0 || (waited < 0 && os::errno() == os::EINTR) {
            return None;
        }
        self.pid = 0;
        if let Some(error) = self.failure.take() {
            return Some(Err(error));
        }
        if waited < 0 || status != 0 {
            return Some(Err("extension worker exited abnormally".into()));
        }
        Some(self.result())
    }

    fn result(&mut self) -> Result<String, String> {
        if self.output.len() < RESPONSE_HEADER || self.output[..4] != *MAGIC {
            return Err("missing extension result".into());
        }
        let status = u32::from_le_bytes(self.output[4..8].try_into().unwrap());
        let count = u32::from_le_bytes(self.output[8..12].try_into().unwrap()) as usize;
        if status > 1 || count > MAX_TEXT_BYTES || self.output.len() != RESPONSE_HEADER + count {
            return Err("invalid extension result length".into());
        }
        let mut bytes = core::mem::take(&mut self.output);
        bytes.drain(..RESPONSE_HEADER);
        let text = String::from_utf8(bytes).map_err(|_| "extension returned invalid UTF-8")?;
        if status == 0 { Ok(text) } else { Err(text) }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // Normal GUI/CLI owners retain cancelled jobs until step reaps them.
        // This fallback handles process teardown without blocking the GUI.
        if self.pid > 0 {
            self.stop();
            unsafe { os::waitpid(self.pid, core::ptr::null_mut(), 1) };
        }
    }
}

fn read_exact(bytes: &mut [u8]) -> Result<(), String> {
    let mut used = 0;
    while used != bytes.len() {
        let count = unsafe { os::read(0, bytes[used..].as_mut_ptr().cast(), bytes.len() - used) };
        if count > 0 {
            used += count as usize;
        } else if count < 0 && os::errno() == os::EINTR {
            continue;
        } else {
            return Err("incomplete extension request".into());
        }
    }
    Ok(())
}

fn write_all(bytes: &[u8]) -> bool {
    let mut used = 0;
    while used < bytes.len() {
        let count = unsafe { os::write(1, bytes[used..].as_ptr().cast(), bytes.len() - used) };
        if count > 0 {
            used += count as usize;
        } else if count < 0 && os::errno() == os::EINTR {
            continue;
        } else {
            return false;
        }
    }
    true
}

fn worker_limits(parent: c_int) -> Result<(), String> {
    // This process is freshly exec'd and has no GUI, PTY or broker descriptors.
    // Limits constrain faulty trusted code; they are not a security sandbox.
    if unsafe {
        prctl(
            1,
            SIGKILL as c_ulong,
            0 as c_ulong,
            0 as c_ulong,
            0 as c_ulong,
        )
    } != 0
        || unsafe { getppid() } != parent
    {
        return Err("extension parent is unavailable".into());
    }
    unsafe {
        signal(14, 0); // SIGALRM / SIG_DFL, also bounds direct helper invocation.
        alarm(3);
    }
    for (resource, limit) in [(0, 1), (4, 0), (7, 64), (9, 256 * 1024 * 1024)] {
        let limit = RLimit {
            current: limit,
            maximum: limit,
        };
        if unsafe { setrlimit(resource, &limit) } != 0 {
            return Err("cannot apply extension worker limits".into());
        }
    }
    Ok(())
}

fn invoke(path: &CStr, mode: Mode, input: &str) -> Result<String, String> {
    // SAFETY: the caller explicitly registered/selected this trusted native
    // library and its dependencies. We require the cooperative v1 contract.
    // Only this disposable worker ever maps or calls plugin code.
    let plugin = unsafe { zplugin::Plugin::load_cooperative(path) }
        .map_err(|error| format!("cannot load extension: {error:?}"))?;
    let result = (|| {
        let mut action = TextActionApi::create(&plugin)
            .map_err(|error| format!("extension interface rejected: {error:?}"))?;
        match mode {
            Mode::Check => {
                let version = plugin.version();
                Ok(format!(
                    "{{\"interface\":\"dev.project-z.ztty.text\",\"interface_version\":1,\"plugin_version\":[{},{},{}],\"unloaded\":true}}\n",
                    version.major, version.minor, version.patch
                ))
            }
            Mode::Run => {
                let mut output = vec![0u8; MAX_TEXT_BYTES];
                let count = action
                    .transform(input, &mut output)
                    .map_err(|error| format!("extension failed: {error:?}"))?;
                let count = usize::try_from(count)
                    .ok()
                    .filter(|count| *count <= output.len())
                    .ok_or("extension output exceeds 1 MiB")?;
                output.truncate(count);
                String::from_utf8(output).map_err(|_| "extension returned invalid UTF-8".into())
            }
        }
        // The instance is destroyed before cooperative library close below.
    })();
    match plugin.try_close() {
        Ok(()) => result,
        Err((error, _plugin)) => Err(format!("extension refused unload: {error:?}")),
    }
}

/// Private exec entry. All errors use the bounded wire; no selected text is logged.
pub fn worker(args: &[&CStr]) -> Result<i32, String> {
    let result = (|| {
        if args.len() != 3 {
            return Err("invalid extension worker arguments".into());
        }
        let parent = args[0]
            .to_str()
            .ok()
            .and_then(|s| s.parse::<c_int>().ok())
            .filter(|pid| *pid > 0)
            .ok_or("invalid extension parent")?;
        worker_limits(parent)?;
        let mode = match args[1].to_bytes() {
            b"check" => Mode::Check,
            b"run" => Mode::Run,
            _ => return Err("invalid extension operation".into()),
        };
        let mut header = [0u8; 8];
        read_exact(&mut header)?;
        let count = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
        if header[..4] != *MAGIC || count > MAX_TEXT_BYTES {
            return Err("invalid extension request length".into());
        }
        let mut input = vec![0u8; count];
        read_exact(&mut input)?;
        let input = core::str::from_utf8(&input).map_err(|_| "extension input must be UTF-8")?;
        invoke(args[2], mode, input)
    })();
    let (status, mut text) = match result {
        Ok(text) => (0u32, text),
        Err(error) => (1u32, error),
    };
    if status != 0 && text.len() > 512 {
        let mut end = 512;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    let mut header = [0u8; RESPONSE_HEADER];
    header[..4].copy_from_slice(MAGIC);
    header[4..8].copy_from_slice(&status.to_le_bytes());
    header[8..].copy_from_slice(&(text.len() as u32).to_le_bytes());
    Ok(if write_all(&header) && write_all(text.as_bytes()) {
        0
    } else {
        1
    })
}

pub fn command(args: &[&CStr]) -> Result<i32, String> {
    if args == [c"--help"] || args == [c"-h"] {
        os::print(
            1,
            b"Usage: ztty plugin check|run /absolute/library.so\n\
check validates the interface and cooperative unload.\n\
run transforms UTF-8 stdin and writes stdout (1 MiB each, 2 seconds).\n\
Native plugins run as your user and must be trusted.\n",
        );
        return Ok(0);
    }
    if args.len() != 2 {
        return Err("usage: ztty plugin check|run /absolute/library.so".into());
    }
    let mode = match args[0].to_bytes() {
        b"check" => Mode::Check,
        b"run" => Mode::Run,
        _ => return Err("expected plugin check or plugin run".into()),
    };
    let path = args[1]
        .to_str()
        .map_err(|_| "extension path must be UTF-8")?;
    let mut input = Vec::new();
    if matches!(mode, Mode::Run) {
        let mut bytes = [0u8; 4096];
        loop {
            let count = unsafe { os::read(0, bytes.as_mut_ptr().cast(), bytes.len()) };
            if count == 0 {
                break;
            }
            if count < 0 && os::errno() == os::EINTR {
                continue;
            }
            if count < 0 {
                return Err("cannot read extension input".into());
            }
            if input.len() + count as usize > MAX_TEXT_BYTES {
                return Err("extension input exceeds 1 MiB".into());
            }
            input.extend_from_slice(&bytes[..count as usize]);
        }
    }
    let text = core::str::from_utf8(&input).map_err(|_| "extension input must be UTF-8")?;
    let mut job = Job::start(path, mode, text)?;
    loop {
        let now = os::milliseconds();
        if let Some(result) = job.step(now) {
            let text = result?;
            return Ok(if write_all(text.as_bytes()) { 0 } else { 1 });
        }
        let mut fd = job.descriptor();
        let timeout = job.next_deadline(now).saturating_sub(now).min(100) as c_int;
        let count = usize::from(fd.is_some());
        let pointer = fd
            .as_mut()
            .map_or(core::ptr::null_mut(), |fd| fd as *mut os::PollFd);
        unsafe {
            os::poll(pointer, count, timeout);
        }
    }
}
