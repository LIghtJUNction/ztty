//! Owned Linux pseudo-terminals without the Rust standard library.
//!
//! [`Pty::spawn`] accepts an already resolved executable path and a complete
//! environment. It performs no PATH lookup, environment mutation, logging,
//! terminal emulation, or UI work. The child side calls only `close`, `execve`,
//! `write`, and `_exit` after `forkpty` has returned.
//!
//! This library uses `alloc` to prepare argument pointers before the fork and
//! links the host C runtime and `libutil`. Read [`Pty::spawn`]'s safety contract.
#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use core::ffi::{CStr, c_char, c_int, c_long, c_void};
use core::fmt;

#[cfg(not(target_os = "linux"))]
compile_error!("zpty currently supports Linux only");

/// Operation that failed at the OS boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Spawn,
    Nonblocking,
    CloseOnExec,
    Read,
    Write,
    Resize,
    Wait,
    Signal,
}

/// An OS error without allocation or a dependency on `std::io`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Error {
    pub operation: Operation,
    pub errno: i32,
}
impl Error {
    fn last(operation: Operation) -> Self {
        Self {
            operation,
            errno: unsafe { *ffi::__errno_location() },
        }
    }
    /// Whether nonblocking I/O should be retried after readiness notification.
    pub const fn is_would_block(self) -> bool {
        self.errno == 11
    }
    /// Whether an OS call was interrupted by a signal.
    pub const fn is_interrupted(self) -> bool {
        self.errno == 4
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: OS error {}", self.operation, self.errno)
    }
}
impl core::error::Error for Error {}

/// Character dimensions and optional pixel dimensions of a pseudo-terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct Size {
    pub rows: u16,
    pub cols: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}
impl Size {
    /// Construct a size without pixel dimensions.
    pub const fn new(cols: u16, rows: u16) -> Self {
        Self {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

/// Reaped child process status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExitStatus(i32);
impl ExitStatus {
    /// Exit code when the child exited normally.
    pub const fn code(self) -> Option<i32> {
        if self.0 & 127 == 0 {
            Some((self.0 >> 8) & 255)
        } else {
            None
        }
    }
    /// Terminating signal when the child did not exit normally.
    pub const fn signal(self) -> Option<i32> {
        let signal = self.0 & 127;
        if signal != 0 && signal != 127 {
            Some(signal)
        } else {
            None
        }
    }
    /// Whether the child exited with code zero.
    pub const fn success(self) -> bool {
        matches!(self.code(), Some(0))
    }
    /// Raw Linux wait status, for callers that need additional wait flags.
    pub const fn raw(self) -> i32 {
        self.0
    }
}

/// An owned nonblocking PTY master and its child process.
///
/// Dropping closes the master, sends the live child session SIGHUP, and attempts
/// a nonblocking reap. Call [`Self::wait`] or [`Self::wait_timeout`] when the
/// process must be joined. A child that deliberately ignores terminal hangup
/// may outlive this object; `Drop` does not kill arbitrary jobs or block.
pub struct Pty {
    fd: i32,
    pid: i32,
    status: Option<ExitStatus>,
}

impl Pty {
    /// Create a session with a controlling terminal and execute a program.
    ///
    /// `program` is passed directly to `execve`; resolve PATH before this call.
    /// `arguments` includes argv[0]. `environment` contains complete `KEY=VALUE`
    /// strings, with no implicit inheritance. `close_in_child` lists inherited
    /// descriptors greater than 2 that the child must close before `execve`.
    /// Arguments and environment pointers are prepared before forking. The
    /// parent master always has `O_NONBLOCK` and `FD_CLOEXEC` when returned.
    ///
    /// An execution failure is reported as child exit code 127 and a short PTY
    /// error message; the parent can observe it with `read` and `wait`.
    ///
    /// # Safety
    ///
    /// The caller must ensure forking is safe for its process and C runtime.
    /// In particular, call before starting additional threads or while the
    /// entire process, signal handlers, allocators, and loaded C libraries are
    /// in a state in which `forkpty` is valid. `forkpty` itself is not specified
    /// as async-signal-safe. No other code may concurrently reap this child,
    /// close a descriptor listed in `close_in_child`, or mutate the C runtime
    /// state used by `forkpty`. Do not use this from a multithreaded host unless
    /// that host has independently established this contract.
    pub unsafe fn spawn(
        program: &CStr,
        arguments: &[&CStr],
        environment: &[&CStr],
        size: Size,
        close_in_child: &[i32],
    ) -> Result<Self, Error> {
        if arguments.is_empty() || size.cols == 0 || size.rows == 0 {
            return Err(Error {
                operation: Operation::Spawn,
                errno: 22,
            });
        }
        let mut argv: Vec<*const c_char> = arguments.iter().map(|s| s.as_ptr()).collect();
        let mut envp: Vec<*const c_char> = environment.iter().map(|s| s.as_ptr()).collect();
        argv.push(core::ptr::null());
        envp.push(core::ptr::null());
        let mut fd = -1;
        let pid = unsafe { ffi::forkpty(&mut fd, core::ptr::null_mut(), core::ptr::null(), &size) };
        if pid < 0 {
            return Err(Error::last(Operation::Spawn));
        }
        if pid == 0 {
            unsafe {
                for &fd in close_in_child {
                    if fd > 2 {
                        ffi::close(fd);
                    }
                }
                ffi::execve(program.as_ptr(), argv.as_ptr(), envp.as_ptr());
                let message = b"zpty: cannot execute requested program\r\n";
                ffi::write(2, message.as_ptr().cast(), message.len());
                ffi::_exit(127);
            }
        }
        let mut pty = Self {
            fd,
            pid,
            status: None,
        };
        let flags = unsafe { ffi::fcntl(fd, 3) };
        let failure = if flags < 0 || unsafe { ffi::fcntl(fd, 4, flags | 0o4000) } < 0 {
            Some(Error::last(Operation::Nonblocking))
        } else if unsafe { ffi::fcntl(fd, 2, 1) } < 0 {
            Some(Error::last(Operation::CloseOnExec))
        } else {
            None
        };
        if let Some(error) = failure {
            unsafe {
                ffi::kill(pid, 9);
            }
            let _ = pty.wait_timeout(100);
            return Err(error);
        }
        Ok(pty)
    }
    /// Borrow the nonblocking master descriptor for poll/epoll registration.
    pub const fn fd(&self) -> i32 {
        self.fd
    }
    /// Child process and session ID.
    pub const fn pid(&self) -> i32 {
        self.pid
    }
    /// Last observed exit status; no OS call is performed.
    pub const fn status(&self) -> Option<ExitStatus> {
        self.status
    }
    /// Read available output. With a nonempty buffer, zero means EOF; Linux
    /// slave-hangup EIO is also reported as EOF.
    pub fn read(&self, bytes: &mut [u8]) -> Result<usize, Error> {
        loop {
            let n = unsafe { ffi::read(self.fd, bytes.as_mut_ptr().cast(), bytes.len()) };
            if n >= 0 {
                return Ok(n as usize);
            }
            let error = Error::last(Operation::Read);
            if error.is_interrupted() {
                continue;
            }
            if error.errno == 5 {
                return Ok(0);
            }
            return Err(error);
        }
    }
    /// Write some input. Preserve any unwritten suffix and retry on POLLOUT.
    pub fn write(&self, bytes: &[u8]) -> Result<usize, Error> {
        loop {
            let n = unsafe { ffi::write(self.fd, bytes.as_ptr().cast(), bytes.len()) };
            if n >= 0 {
                return Ok(n as usize);
            }
            let error = Error::last(Operation::Write);
            if error.is_interrupted() {
                continue;
            }
            return Err(error);
        }
    }
    /// Update dimensions and deliver SIGWINCH to the foreground process group.
    pub fn resize(&self, size: Size) -> Result<(), Error> {
        if size.cols == 0 || size.rows == 0 {
            return Err(Error {
                operation: Operation::Resize,
                errno: 22,
            });
        }
        if unsafe { ffi::ioctl(self.fd, 0x5414, &size) } < 0 {
            Err(Error::last(Operation::Resize))
        } else {
            Ok(())
        }
    }
    /// Reap only this object's child, without waiting.
    pub fn reap(&mut self) -> Result<Option<ExitStatus>, Error> {
        if self.status.is_some() {
            return Ok(self.status);
        }
        let mut status = 0;
        loop {
            let result = unsafe { ffi::waitpid(self.pid, &mut status, 1) };
            if result == 0 {
                return Ok(None);
            }
            if result == self.pid {
                self.status = Some(ExitStatus(status));
                return Ok(self.status);
            }
            let error = Error::last(Operation::Wait);
            if error.is_interrupted() {
                continue;
            }
            return Err(error);
        }
    }
    /// Join this child's process. This deliberately waits without a timeout.
    pub fn wait(&mut self) -> Result<ExitStatus, Error> {
        if let Some(status) = self.status {
            return Ok(status);
        }
        let mut raw = 0;
        loop {
            let result = unsafe { ffi::waitpid(self.pid, &mut raw, 0) };
            if result == self.pid {
                let status = ExitStatus(raw);
                self.status = Some(status);
                return Ok(status);
            }
            let error = Error::last(Operation::Wait);
            if !error.is_interrupted() {
                return Err(error);
            }
        }
    }
    /// Join for at most approximately `timeout_ms`, measured with a monotonic clock.
    pub fn wait_timeout(&mut self, timeout_ms: u32) -> Result<Option<ExitStatus>, Error> {
        let end = milliseconds()?.saturating_add(timeout_ms as u64);
        loop {
            if let Some(status) = self.reap()? {
                return Ok(Some(status));
            }
            let now = milliseconds()?;
            if now >= end {
                return Ok(None);
            }
            unsafe {
                ffi::poll(core::ptr::null_mut(), 0, (end - now).min(2) as i32);
            }
        }
    }
    /// Send SIGHUP to the child session. Already exited sessions are accepted.
    pub fn hangup(&self) -> Result<(), Error> {
        if self.status.is_some() {
            return Ok(());
        }
        if unsafe { ffi::kill(-self.pid, 1) } < 0 {
            let error = Error::last(Operation::Signal);
            if error.errno != 3 {
                return Err(error);
            }
        }
        Ok(())
    }
}
impl Drop for Pty {
    fn drop(&mut self) {
        unsafe {
            ffi::close(self.fd);
        }
        let _ = self.reap();
        let _ = self.hangup();
        let _ = self.reap();
    }
}

fn milliseconds() -> Result<u64, Error> {
    let mut time = ffi::TimeSpec {
        seconds: 0,
        nanoseconds: 0,
    };
    if unsafe { ffi::clock_gettime(1, &mut time) } < 0 {
        return Err(Error::last(Operation::Wait));
    }
    Ok((time.seconds as u64) * 1000 + (time.nanoseconds as u64) / 1_000_000)
}

mod ffi {
    use super::*;
    #[repr(C)]
    pub struct TimeSpec {
        pub seconds: c_long,
        pub nanoseconds: c_long,
    }
    #[link(name = "c")]
    unsafe extern "C" {
        pub fn read(fd: c_int, data: *mut c_void, size: usize) -> isize;
        pub fn write(fd: c_int, data: *const c_void, size: usize) -> isize;
        pub fn close(fd: c_int) -> c_int;
        pub fn poll(fds: *mut c_void, count: usize, timeout: c_int) -> c_int;
        pub fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
        pub fn ioctl(fd: c_int, request: usize, ...) -> c_int;
        pub fn execve(
            file: *const c_char,
            argv: *const *const c_char,
            environment: *const *const c_char,
        ) -> c_int;
        pub fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
        pub fn kill(pid: c_int, signal: c_int) -> c_int;
        pub fn _exit(status: c_int) -> !;
        pub fn __errno_location() -> *mut c_int;
        pub fn clock_gettime(clock: c_int, time: *mut TimeSpec) -> c_int;
    }
    #[link(name = "util")]
    unsafe extern "C" {
        pub fn forkpty(
            master: *mut c_int,
            name: *mut c_char,
            term: *const c_void,
            size: *const Size,
        ) -> c_int;
    }
}
