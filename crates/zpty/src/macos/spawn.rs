//! Darwin's spawn file actions run before SETSID. A fresh executable image
//! performs TIOCSCTTY afterwards; no Rust child code runs after a host fork.
use super::{Error, Operation, Pty, Size, ffi, milliseconds};
use alloc::{ffi::CString, vec::Vec};
use core::ffi::{CStr, c_char, c_int};

const HELPER_ARGUMENT: &CStr = c"--zpty-spawn-helper-v1";
const HANDSHAKE_TIMEOUT_MS: u64 = 5_000;
const FRAME_MAGIC: &[u8; 4] = b"ZPT1";
const EINVAL: i32 = 22;
const EIO: i32 = 5;
const ETIMEDOUT: i32 = 60;
// Apple XNU f6217f8: bsd/sys/{fcntl,spawn,ttycom}.h. The native SDK test
// checks these declarations and constants on both supported Darwin targets.
const O_RDWR: i32 = 2;
const O_NONBLOCK: i32 = 0x0004;
const O_NOCTTY: i32 = 0x0002_0000;
const O_CLOEXEC: i32 = 0x0100_0000;
const F_DUPFD_CLOEXEC: i32 = 67;
const AT_FDCWD: i32 = -2;
const AT_EACCESS: i32 = 0x0010;
const X_OK: i32 = 1;
const TIOCPTYGNAME: u64 = 0x4080_7453;
const TIOCSCTTY: u64 = 0x2000_7461;
const TIOCSWINSZ: u64 = 0x8008_7467;
const POSIX_SPAWN_SETSIGDEF: i16 = 0x0004;
const POSIX_SPAWN_SETSIGMASK: i16 = 0x0008;
const POSIX_SPAWN_SETSID: i16 = 0x0400;
const POSIX_SPAWN_CLOEXEC_DEFAULT: i16 = 0x4000;

fn error(errno: i32) -> Error {
    Error {
        operation: Operation::Spawn,
        errno,
    }
}

fn syscall(result: c_int) -> Result<(), Error> {
    if result < 0 {
        Err(Error::last(Operation::Spawn))
    } else {
        Ok(())
    }
}

// The posix_spawn family returns an error number; it does not report failure
// using -1/errno like open, fcntl and ioctl.
fn spawn_call(result: c_int) -> Result<(), Error> {
    if result == 0 {
        Ok(())
    } else {
        Err(error(result))
    }
}

struct Descriptor(c_int);

impl Descriptor {
    fn new(fd: c_int) -> Result<Self, Error> {
        if fd < 0 {
            Err(Error::last(Operation::Spawn))
        } else {
            Ok(Self(fd))
        }
    }

    fn close_on_exec(&self) -> Result<(), Error> {
        let flags = unsafe { ffi::fcntl(self.0, 1) };
        syscall(flags)?;
        syscall(unsafe { ffi::fcntl(self.0, 2, flags | 1) })
    }

    fn nonblocking(&self) -> Result<(), Error> {
        let flags = unsafe { ffi::fcntl(self.0, 3) };
        syscall(flags)?;
        syscall(unsafe { ffi::fcntl(self.0, 4, flags | O_NONBLOCK) })
    }

    // A host may have closed any of stdin/stdout/stderr. Keep all private
    // sources above 2 before constructing actions which replace those slots.
    fn private(self) -> Result<Self, Error> {
        if self.0 > 2 {
            Ok(self)
        } else {
            Self::new(unsafe { ffi::fcntl(self.0, F_DUPFD_CLOEXEC, 3) })
        }
    }

    fn into_raw(self) -> c_int {
        let fd = self.0;
        core::mem::forget(self);
        fd
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe { ffi::close(self.0) };
    }
}

fn error_pipe() -> Result<(Descriptor, Descriptor), Error> {
    let mut pair = [-1; 2];
    syscall(unsafe { ffi::pipe(pair.as_mut_ptr()) })?;
    // Own both descriptors before any fallible configuration.
    let reader = Descriptor(pair[0]);
    let writer = Descriptor(pair[1]);
    // Darwin has pipe(), not pipe2(O_CLOEXEC). Concurrent calls to our spawn
    // API are isolated by CLOEXEC_DEFAULT. Unrelated fork/exec users must
    // establish their own descriptor-inheritance policy for this short gap.
    reader.close_on_exec()?;
    writer.close_on_exec()?;
    let reader = reader.private()?;
    let writer = writer.private()?;
    reader.nonblocking()?;
    writer.nonblocking()?;
    Ok((reader, writer))
}

struct SpawnSetup {
    actions: *mut core::ffi::c_void,
    attributes: *mut core::ffi::c_void,
}

impl SpawnSetup {
    fn new() -> Result<Self, Error> {
        let mut actions = core::ptr::null_mut();
        spawn_call(unsafe { ffi::posix_spawn_file_actions_init(&mut actions) })?;
        let mut setup = Self {
            actions,
            attributes: core::ptr::null_mut(),
        };
        let mut attributes = core::ptr::null_mut();
        spawn_call(unsafe { ffi::posix_spawnattr_init(&mut attributes) })?;
        setup.attributes = attributes;
        Ok(setup)
    }
}

impl Drop for SpawnSetup {
    fn drop(&mut self) {
        unsafe {
            if !self.actions.is_null() {
                ffi::posix_spawn_file_actions_destroy(&mut self.actions);
            }
            if !self.attributes.is_null() {
                ffi::posix_spawnattr_destroy(&mut self.attributes);
            }
        }
    }
}

struct SpawnedChild(c_int);

impl Drop for SpawnedChild {
    fn drop(&mut self) {
        // Only this directly owned child is waited for or signalled. The host
        // must not concurrently reap it, as with the existing Pty ownership API.
        let mut status = 0;
        let running = loop {
            let result = unsafe { ffi::waitpid(self.0, &mut status, 1) };
            if result == 0 {
                break true;
            }
            if result >= 0 || !Error::last(Operation::Wait).is_interrupted() {
                break false;
            }
        };
        if running {
            unsafe { ffi::kill(self.0, 9) };
            loop {
                let result = unsafe { ffi::waitpid(self.0, &mut status, 0) };
                if result >= 0 || !Error::last(Operation::Wait).is_interrupted() {
                    break;
                }
            }
        }
    }
}

/// Resolve this running executable independently of argv[0] and the current
/// directory. The result is an absolute kernel-reported path, suitable as the
/// helper for [`Pty::spawn_with_helper`] when startup calls [`run_spawn_helper`].
/// An executable removed from the filesystem can fail resolution or spawning;
/// this function does not fall back to PATH or a different executable.
pub fn current_executable() -> Result<CString, Error> {
    // PROC_PIDPATHINFO_MAXSIZE = 4 * MAXPATHLEN in Apple's proc_info.h.
    let mut path = [0_u8; 4096];
    let length =
        unsafe { ffi::proc_pidpath(ffi::getpid(), path.as_mut_ptr().cast(), path.len() as u32) };
    if length <= 0 {
        return Err(Error::last(Operation::Spawn));
    }
    if length as usize >= path.len() || path[0] != b'/' {
        return Err(error(EIO));
    }
    CStr::from_bytes_with_nul(&path[..=length as usize])
        .map(CString::from)
        .map_err(|_| error(EIO))
}

impl Pty {
    /// Create a Darwin PTY from a multithreaded host through `posix_spawn`.
    ///
    /// `helper` is an absolute executable path whose entry point calls
    /// [`run_spawn_helper`] before configuration, cwd changes, UI initialization
    /// or starting host threads. [`current_executable`] resolves such a host
    /// independently of argv[0]. `program`, argv (including argv[0]) and the
    /// complete environment are passed without PATH lookup or text conversion.
    ///
    /// The master is nonblocking and close-on-exec. Only slave descriptors
    /// 0/1/2 and a private error writer reach the helper. The writer closes on
    /// the target exec. No helper remains after success: it becomes the target
    /// program with the same child PID, session and process group.
    ///
    /// The helper path is checked for execute access using the effective IDs
    /// before allocating a PTY or creating a child. This is not an atomic
    /// check-and-exec guarantee: the path or its permissions can change after
    /// the check, and executable format or script-interpreter errors can still
    /// be returned by `posix_spawn`. Its original OS error is preserved.
    ///
    /// When `posix_spawn` itself fails, Darwin has not handed the caller a
    /// child PID. Its kernel disposes of any internally created failed process;
    /// it can remain visible to process-wide child queries during that disposal.
    /// This API does not guess its PID or reap unrelated host children.
    /// After a successful spawn hands over a PID, controlling-terminal,
    /// handshake and target-exec failures clean up and reap that owned child
    /// before returning the original error.
    ///
    /// A fixed ready frame must precede the EOF used for exec handoff. The
    /// handshake allows five seconds after `posix_spawn` returns; kernel process
    /// creation and kill/reap completion are not hard-real-time bounded.
    /// A missing or invalid handshake fails.
    ///
    /// Concurrent calls to this API cannot inherit each other's private fds:
    /// `POSIX_SPAWN_CLOEXEC_DEFAULT` allows only the explicit file actions.
    /// Darwin's `pipe` followed by `fcntl` has a short close-on-exec setup gap;
    /// unrelated fork/exec code must manage its own inheritance policy. The
    /// caller must retain exclusive wait/reap ownership of the returned child
    /// and must not enable automatic SIGCHLD reaping. This method does not
    /// change the process-wide safety contract of the legacy unsafe `spawn`.
    pub fn spawn_with_helper(
        helper: &CStr,
        program: &CStr,
        arguments: &[&CStr],
        environment: &[&CStr],
        size: Size,
    ) -> Result<Self, Error> {
        if arguments.is_empty()
            || size.cols == 0
            || size.rows == 0
            || !helper.to_bytes().starts_with(b"/")
        {
            return Err(error(EINVAL));
        }
        // Reject known unusable helpers before Darwin creates an internal
        // failed-spawn process without returning its PID to the caller.
        syscall(unsafe { ffi::faccessat(AT_FDCWD, helper.as_ptr(), X_OK, AT_EACCESS) })?;
        let master = Descriptor::new(unsafe {
            ffi::posix_openpt(O_RDWR | O_NOCTTY | O_CLOEXEC | O_NONBLOCK)
        })?
        .private()?;
        syscall(unsafe { ffi::grantpt(master.0) })?;
        syscall(unsafe { ffi::unlockpt(master.0) })?;
        let mut name = [0_u8; 128];
        syscall(unsafe { ffi::ioctl(master.0, TIOCPTYGNAME, name.as_mut_ptr()) })?;
        let slave = CStr::from_bytes_until_nul(&name).map_err(|_| error(EIO))?;
        let (reader, writer) = error_pipe()?;
        let mut setup = SpawnSetup::new()?;
        spawn_call(unsafe {
            ffi::posix_spawn_file_actions_addopen(
                &mut setup.actions,
                0,
                slave.as_ptr(),
                O_RDWR | O_NOCTTY,
                0,
            )
        })?;
        for fd in [1, 2] {
            spawn_call(unsafe {
                ffi::posix_spawn_file_actions_adddup2(&mut setup.actions, 0, fd)
            })?;
        }
        // writer is always above 2, so the preceding actions cannot replace it.
        spawn_call(unsafe {
            ffi::posix_spawn_file_actions_addinherit_np(&mut setup.actions, writer.0)
        })?;
        let mut mask = 0_u32;
        let mut defaults = 0_u32;
        syscall(unsafe { ffi::sigemptyset(&mut mask) })?;
        syscall(unsafe { ffi::sigfillset(&mut defaults) })?;
        syscall(unsafe { ffi::sigdelset(&mut defaults, 9) })?;
        syscall(unsafe { ffi::sigdelset(&mut defaults, 17) })?;
        spawn_call(unsafe { ffi::posix_spawnattr_setsigmask(&mut setup.attributes, &mask) })?;
        spawn_call(unsafe {
            ffi::posix_spawnattr_setsigdefault(&mut setup.attributes, &defaults)
        })?;
        spawn_call(unsafe {
            ffi::posix_spawnattr_setflags(
                &mut setup.attributes,
                POSIX_SPAWN_SETSID
                    | POSIX_SPAWN_CLOEXEC_DEFAULT
                    | POSIX_SPAWN_SETSIGDEF
                    | POSIX_SPAWN_SETSIGMASK,
            )
        })?;
        let mut numbers = [[0_u8; 12]; 5];
        let values = [
            writer.0 as u32,
            size.cols as u32,
            size.rows as u32,
            size.pixel_width as u32,
            size.pixel_height as u32,
        ];
        let mut starts = [0; 5];
        for index in 0..numbers.len() {
            starts[index] = number_text(values[index], &mut numbers[index]);
        }
        let mut argv: Vec<*const c_char> = Vec::with_capacity(arguments.len() + 9);
        argv.extend([helper.as_ptr(), HELPER_ARGUMENT.as_ptr()]);
        argv.extend(
            numbers
                .iter()
                .zip(starts)
                .map(|(number, start)| number[start..].as_ptr().cast()),
        );
        argv.push(program.as_ptr());
        argv.extend(arguments.iter().map(|arg| arg.as_ptr()));
        argv.push(core::ptr::null());
        let mut envp: Vec<*const c_char> = environment.iter().map(|arg| arg.as_ptr()).collect();
        envp.push(core::ptr::null());
        let mut pid = 0;
        spawn_call(unsafe {
            ffi::posix_spawn(
                &mut pid,
                helper.as_ptr(),
                &setup.actions,
                &setup.attributes,
                argv.as_ptr(),
                envp.as_ptr(),
            )
        })?;
        let child = SpawnedChild(pid);
        drop(writer);
        handshake(reader.0)?;
        core::mem::forget(child);
        Ok(Self {
            fd: master.into_raw(),
            pid,
            status: None,
        })
    }
}

fn clock() -> Result<u64, Error> {
    milliseconds().map_err(|failure| error(failure.errno))
}

fn frame_errno(frame: &[u8]) -> Result<i32, Error> {
    if &frame[..4] != FRAME_MAGIC {
        return Err(error(EIO));
    }
    let errno = i32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]);
    if errno < 0 {
        Err(error(EIO))
    } else {
        Ok(errno)
    }
}

fn handshake(fd: c_int) -> Result<(), Error> {
    let deadline = clock()?.saturating_add(HANDSHAKE_TIMEOUT_MS);
    let mut bytes = [0_u8; 16];
    let mut used = 0;
    loop {
        if clock()? >= deadline {
            return Err(error(ETIMEDOUT));
        }
        let count = unsafe { ffi::read(fd, bytes[used..].as_mut_ptr().cast(), bytes.len() - used) };
        if count > 0 {
            used += count as usize;
            if used >= 8 {
                let errno = frame_errno(&bytes[..8])?;
                if errno != 0 {
                    return Err(error(errno));
                }
            }
            if used == 16 {
                let errno = frame_errno(&bytes[8..])?;
                return Err(error(if errno == 0 { EIO } else { errno }));
            }
            continue;
        }
        if count == 0 {
            return if used == 8 { Ok(()) } else { Err(error(EIO)) };
        }
        let failure = Error::last(Operation::Spawn);
        if failure.is_interrupted() {
            continue;
        }
        if !failure.is_would_block() {
            return Err(failure);
        }
        let now = clock()?;
        if now >= deadline {
            return Err(error(ETIMEDOUT));
        }
        let mut descriptor = ffi::PollFd {
            fd,
            events: 1,
            revents: 0,
        };
        let result = unsafe {
            ffi::poll(
                (&mut descriptor as *mut ffi::PollFd).cast(),
                1,
                (deadline - now).min(i32::MAX as u64) as i32,
            )
        };
        if result < 0 {
            let failure = Error::last(Operation::Spawn);
            if !failure.is_interrupted() {
                return Err(failure);
            }
        }
    }
}

fn number_text(mut value: u32, number: &mut [u8; 12]) -> usize {
    let mut start = number.len() - 1;
    loop {
        start -= 1;
        number[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            return start;
        }
    }
}

fn unsigned_number(argument: &CStr) -> Option<u32> {
    let bytes = argument.to_bytes();
    if bytes.is_empty() {
        return None;
    }
    let mut value = 0_u32;
    for &byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add((byte - b'0') as u32)?;
    }
    Some(value)
}

fn descriptor_number(argument: &CStr) -> Option<c_int> {
    let value = c_int::try_from(unsigned_number(argument)?).ok()?;
    (value > 2).then_some(value)
}

fn dimension(argument: &CStr) -> Result<u16, Error> {
    unsigned_number(argument)
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| error(EINVAL))
}

fn send_frame(fd: c_int, errno: i32) -> bool {
    let mut bytes = [0_u8; 8];
    bytes[..4].copy_from_slice(FRAME_MAGIC);
    bytes[4..].copy_from_slice(&errno.to_le_bytes());
    let mut written = 0;
    while written < bytes.len() {
        let n = unsafe { ffi::write(fd, bytes[written..].as_ptr().cast(), bytes.len() - written) };
        if n > 0 {
            written += n as usize;
        } else if n == 0 || !Error::last(Operation::Spawn).is_interrupted() {
            return false;
        }
    }
    true
}

/// Dispatch the internal helper request at the beginning of a fresh process.
///
/// Pass the complete argv including argv[0], before configuration, cwd changes,
/// UI initialization or host threads. Ordinary launches return immediately.
/// A helper request either execs the supplied program or exits with code 127
/// after reporting the exact failure through the private error descriptor.
/// This is an executable-entry API, not a way to convert an existing live UI
/// process into a terminal child. It does not fork or create a second helper.
pub fn run_spawn_helper(arguments: &[&CStr]) {
    if arguments.get(1).copied() != Some(HELPER_ARGUMENT) {
        return;
    }
    let fd = arguments.get(2).and_then(|arg| descriptor_number(arg));
    let Some(fd) = fd else {
        unsafe { ffi::_exit(127) }
    };
    let result = helper_exec(fd, arguments);
    if let Err(failure) = result {
        send_frame(fd, failure.errno);
    }
    unsafe { ffi::_exit(127) }
}

fn helper_exec(fd: c_int, arguments: &[&CStr]) -> Result<(), Error> {
    if arguments.len() < 9 {
        return Err(error(EINVAL));
    }
    let flags = unsafe { ffi::fcntl(fd, 1) };
    syscall(flags)?;
    syscall(unsafe { ffi::fcntl(fd, 2, flags | 1) })?;
    let pid = unsafe { ffi::getpid() };
    if unsafe { ffi::getsid(0) } != pid || unsafe { ffi::getpgrp() } != pid {
        return Err(error(1));
    }
    syscall(unsafe { ffi::ioctl(0, TIOCSCTTY, core::ptr::null::<core::ffi::c_void>()) })?;
    let foreground = unsafe { ffi::tcgetpgrp(0) };
    syscall(foreground)?;
    if foreground != pid {
        return Err(error(1));
    }
    let size = Size {
        cols: dimension(arguments[3])?,
        rows: dimension(arguments[4])?,
        pixel_width: dimension(arguments[5])?,
        pixel_height: dimension(arguments[6])?,
    };
    if size.cols == 0 || size.rows == 0 {
        return Err(error(EINVAL));
    }
    // XNU rejects extended master ioctls until the first slave open. Its
    // file-action open has now initialized the slave, so set all dimensions
    // here before acknowledging readiness or executing the target.
    syscall(unsafe { ffi::ioctl(0, TIOCSWINSZ, &size) })?;
    let mut argv: Vec<*const c_char> = arguments[8..].iter().map(|arg| arg.as_ptr()).collect();
    argv.push(core::ptr::null());
    if !send_frame(fd, 0) {
        return Err(error(EIO));
    }
    unsafe {
        ffi::execve(
            arguments[7].as_ptr(),
            argv.as_ptr(),
            (*ffi::_NSGetEnviron()).cast(),
        );
    }
    Err(Error::last(Operation::Spawn))
}
