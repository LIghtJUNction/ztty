//! Linux same-process executable replacement. The old loop remains intact until
//! a pinned new executable has decoded the sealed checkpoint successfully.
use crate::{
    broker,
    json::{self, Value},
    os, session,
};
use alloc::{ffi::CString, format, string::String, vec, vec::Vec};
use core::ffi::{CStr, c_char, c_int, c_void};

pub const WAIT: &str = "__runtime_reload__";
const MAGIC: &[u8; 8] = b"ZTTYRL01";
const VALIDATED: &[u8; 8] = b"ZTTYRV01";
const MAX_BYTES: usize = 128 * 1024 * 1024;
const MAX_META: usize = 32 * 1024 * 1024;
const FD_CLOEXEC: i32 = 1;

unsafe extern "C" {
    fn memfd_create(name: *const c_char, flags: u32) -> i32;
    fn lseek(fd: i32, offset: i64, whence: i32) -> i64;
    fn execveat(
        fd: i32,
        path: *const c_char,
        argv: *const *const c_char,
        env: *const *const c_char,
        flags: i32,
    ) -> i32;
    fn opendir(path: *const c_char) -> *mut c_void;
    fn readdir(dir: *mut c_void) -> *mut Dirent;
    fn closedir(dir: *mut c_void) -> c_int;
    fn pipe2(descriptors: *mut i32, flags: i32) -> i32;
    fn close_range(first: u32, last: u32, flags: i32) -> i32;
    fn getrandom(buffer: *mut c_void, length: usize, flags: u32) -> isize;
}

#[repr(C)]
struct Dirent {
    ino: u64,
    offset: i64,
    len: u16,
    kind: u8,
    name: [c_char; 256],
}

pub fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("reload: invalid {key}"))
}
pub fn number(value: &Value, key: &str) -> Result<u64, String> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("reload: invalid {key}"))
}
pub fn boolean(value: &Value, key: &str) -> Result<bool, String> {
    value
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("reload: invalid {key}"))
}
pub fn list<'a>(value: &'a Value, key: &str) -> Result<&'a [Value], String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("reload: invalid {key}"))
}
pub fn bytes(value: &Value, key: &str, limit: usize) -> Result<Vec<u8>, String> {
    let input = text(value, key)?.as_bytes();
    if input.len() % 2 != 0 || input.len() / 2 > limit {
        return Err(format!("reload: {key} exceeds its byte budget"));
    }
    let digit = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    };
    input
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            Ok((digit(pair[0]).ok_or("reload: invalid hex")? << 4)
                | digit(pair[1]).ok_or("reload: invalid hex")?)
        })
        .collect()
}
pub fn descriptor(value: &Value, key: &str) -> Result<i32, String> {
    i32::try_from(number(value, key)?)
        .ok()
        .filter(|fd| *fd > 2)
        .ok_or_else(|| format!("reload: invalid descriptor {key}"))
}

pub fn command(args: &[&CStr]) -> Result<i32, String> {
    let mut request = json::object([("op", Value::from("reload"))]);
    match args {
        [] => {}
        [flag, path] if flag.to_bytes() == b"--binary" && path.to_bytes().starts_with(b"/") => {
            request.set("_reload_image", Value::from(broker::hex(path.to_bytes())));
        }
        _ => return Err("usage: ztty reload [--binary /absolute/path/to/ztty]".into()),
    }
    let result = broker::request(&request)?;
    os::print(1, json::stringify(&result).as_bytes());
    os::print(1, b"\n");
    Ok(if result.get("window_error").is_some() {
        1
    } else {
        0
    })
}

fn read_snapshot(fd: i32) -> Result<Vec<u8>, String> {
    // The checkpoint is sealed before either validator or replacement sees it.
    let seals = unsafe { os::fcntl(fd, 1034) };
    if seals < 0 || seals & 15 != 15 {
        return Err("reload checkpoint is not sealed".into());
    }
    let size = unsafe { lseek(fd, 0, 2) };
    if size < 16 || size as usize > MAX_BYTES || unsafe { lseek(fd, 0, 0) } < 0 {
        return Err("reload checkpoint size is invalid".into());
    }
    let mut bytes = vec![0; size as usize];
    let mut used = 0;
    while used < bytes.len() {
        let n = unsafe { os::read(fd, bytes[used..].as_mut_ptr().cast(), bytes.len() - used) };
        if n > 0 {
            used += n as usize;
        } else if n < 0 && os::errno() == os::EINTR {
            continue;
        } else {
            return Err("reload checkpoint read failed".into());
        }
    }
    Ok(bytes)
}

fn decode(bytes: &[u8]) -> Result<(Value, Vec<zterm::Terminal>), String> {
    if bytes.get(..8) != Some(MAGIC.as_slice()) {
        return Err("unsupported reload checkpoint format".into());
    }
    let mut at = 8;
    let mut field = || -> Result<&[u8], String> {
        let raw: [u8; 8] = bytes
            .get(at..at + 8)
            .ok_or("truncated reload checkpoint")?
            .try_into()
            .map_err(|_| "invalid reload field")?;
        at += 8;
        let length =
            usize::try_from(u64::from_le_bytes(raw)).map_err(|_| "reload field overflow")?;
        let end = at.checked_add(length).ok_or("reload field overflow")?;
        let value = bytes.get(at..end).ok_or("truncated reload field")?;
        at = end;
        Ok(value)
    };
    let raw_meta = field()?;
    if raw_meta.len() > MAX_META {
        return Err("reload metadata budget exceeded".into());
    }
    let meta = json::parse_checkpoint(raw_meta).map_err(String::from)?;
    let count = list(&meta, "sessions")?.len();
    if count > 128 {
        return Err("reload session budget exceeded".into());
    }
    let mut terminals = Vec::with_capacity(count);
    for _ in 0..count {
        terminals.push(zterm::Terminal::restore_checkpoint(field()?).map_err(String::from)?);
    }
    if at != bytes.len() {
        return Err("trailing reload checkpoint data".into());
    }
    session::validate_reload(&meta, &terminals)?;
    broker::Server::validate_reload(meta.get("server").ok_or("missing reload server")?)?;
    Ok((meta, terminals))
}

pub fn internal(args: &[&CStr], restore: bool) -> Result<i32, String> {
    if args.len() != if restore { 1 } else { 3 } {
        return Err("invalid internal reload invocation".into());
    }
    let fd = args[0]
        .to_str()
        .ok()
        .and_then(|s| s.parse::<i32>().ok())
        .filter(|fd| *fd > 2)
        .ok_or("invalid reload checkpoint descriptor")?;
    let bytes = read_snapshot(fd)?;
    let (meta, terminals) = decode(&bytes)?;
    drop(bytes);
    if !restore {
        let reply = args[1]
            .to_str()
            .ok()
            .and_then(|s| s.parse::<i32>().ok())
            .filter(|reply| *reply > 2 && *reply != fd)
            .ok_or("invalid reload validation reply descriptor")?;
        let nonce = args[2].to_bytes();
        if nonce.len() != 32 || nonce.iter().any(|b| !b.is_ascii_hexdigit()) {
            return Err("invalid reload validation nonce".into());
        }
        // A successful exit alone is insufficient: an unrelated executable
        // such as /bin/true also exits zero without validating any state.
        write_all(reply, VALIDATED)?;
        write_all(reply, nonce)?;
        return Ok(0);
    }
    if number(&meta, "broker_pid")? != unsafe { os::getpid() } as u64 {
        return Err("reload must preserve the broker process identity".into());
    }
    // Restore close-on-exec before any shell can be started by the new loop.
    let inherited = session::reload_fds(&meta)?;
    if inherited.contains(&fd) || inherited.iter().any(|fd| unsafe { os::fcntl(*fd, 1) } < 0) {
        return Err("inherited reload descriptors are incomplete".into());
    }
    for inherited in inherited {
        if unsafe { os::fcntl(inherited, 2, FD_CLOEXEC) } < 0 {
            return Err("inherited reload descriptor is unavailable".into());
        }
    }
    unsafe {
        os::close(fd);
    }
    session::serve_reloaded(meta, terminals).map(|()| 0)
}

fn write_all(fd: i32, mut bytes: &[u8]) -> Result<(), String> {
    while !bytes.is_empty() {
        let n = unsafe { os::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if n > 0 {
            bytes = &bytes[n as usize..];
        } else if n < 0 && os::errno() == os::EINTR {
            continue;
        } else {
            return Err("cannot write reload checkpoint".into());
        }
    }
    Ok(())
}

fn open_descriptors() -> Result<Vec<(i32, i32)>, String> {
    let directory = unsafe { opendir(c"/proc/self/fd".as_ptr()) };
    if directory.is_null() {
        return Err("cannot enumerate reload descriptors".into());
    }
    let mut result = Vec::new();
    loop {
        let entry = unsafe { readdir(directory) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).name.as_ptr()) };
        if let Some(fd) = name
            .to_str()
            .ok()
            .and_then(|s| s.parse::<i32>().ok())
            .filter(|fd| *fd > 2)
        {
            let flags = unsafe { os::fcntl(fd, 1) };
            if flags >= 0 {
                result.push((fd, flags));
            }
        }
    }
    unsafe {
        closedir(directory);
    }
    result.retain(|(fd, _)| unsafe { os::fcntl(*fd, 1) } >= 0);
    Ok(result)
}

pub fn execute(
    target: &CStr,
    meta: Value,
    terminals: Vec<Vec<u8>>,
    inherited: &[i32],
) -> Result<(), String> {
    // Nonblocking open also makes an accidental FIFO path fail preparation
    // instead of parking the only broker loop indefinitely.
    let image = broker::Fd(unsafe { os::open(target.as_ptr(), 0o2000000 | 0o4000) });
    if image.0 < 0 {
        return Err(format!(
            "cannot open reload executable (OS error {})",
            os::errno()
        ));
    }
    let mut magic = [0u8; 4];
    let count = loop {
        let count = unsafe { os::read(image.0, magic.as_mut_ptr().cast(), magic.len()) };
        if count < 0 && os::errno() == os::EINTR {
            continue;
        }
        break count;
    };
    if count != 4 || &magic != b"\x7fELF" {
        return Err("reload target must be a native ztty executable".into());
    }
    let state = broker::Fd(unsafe { memfd_create(c"ztty-reload".as_ptr(), 3) });
    if state.0 < 0 {
        return Err("cannot create private reload checkpoint".into());
    }
    let metadata = json::stringify(&meta).into_bytes();
    let total = terminals
        .iter()
        .try_fold(16usize + metadata.len(), |n, b| n.checked_add(8 + b.len()))
        .ok_or("reload checkpoint size overflow")?;
    if total > MAX_BYTES || metadata.len() > MAX_META {
        return Err("reload checkpoint exceeds its memory budget".into());
    }
    write_all(state.0, MAGIC)?;
    write_all(state.0, &(metadata.len() as u64).to_le_bytes())?;
    write_all(state.0, &metadata)?;
    for terminal in &terminals {
        write_all(state.0, &(terminal.len() as u64).to_le_bytes())?;
        write_all(state.0, terminal)?;
    }
    drop(terminals);
    drop(metadata);
    if unsafe { os::fcntl(state.0, 1033, 15) } < 0 {
        return Err("cannot seal reload checkpoint".into());
    }
    let fd_text = CString::new(format!("{}", state.0)).map_err(|_| "invalid reload descriptor")?;
    let mut ack = [-1; 2];
    if unsafe { pipe2(ack.as_mut_ptr(), 0o2000000 | 0o4000) } < 0 {
        return Err("cannot create reload validation handshake".into());
    }
    let ack_read = broker::Fd(ack[0]);
    let ack_write = broker::Fd(ack[1]);
    let ack_text =
        CString::new(format!("{}", ack_write.0)).map_err(|_| "invalid validation descriptor")?;
    let mut nonce = [0u8; 16];
    let mut used = 0;
    while used < nonce.len() {
        let count = unsafe { getrandom(nonce[used..].as_mut_ptr().cast(), nonce.len() - used, 1) };
        if count > 0 {
            used += count as usize;
        } else if count < 0 && os::errno() == os::EINTR {
            continue;
        } else {
            return Err("cannot obtain reload validation nonce".into());
        }
    }
    let nonce = CString::new(broker::hex(&nonce)).map_err(|_| "invalid reload nonce")?;
    let environment = os::environment();
    let mut env: Vec<*const c_char> = environment.iter().map(|s| s.as_ptr()).collect();
    env.push(core::ptr::null());
    let check = [
        c"ztty".as_ptr(),
        c"__validate-reload".as_ptr(),
        fd_text.as_ptr(),
        ack_text.as_ptr(),
        nonce.as_ptr(),
        core::ptr::null(),
    ];
    let resume = [
        c"ztty".as_ptr(),
        c"__restore-reload".as_ptr(),
        fd_text.as_ptr(),
        core::ptr::null(),
    ];
    let mut validation_fds = [image.0 as u32, state.0 as u32, ack_write.0 as u32];
    validation_fds.sort_unstable();
    let pid = unsafe { os::fork() };
    if pid < 0 {
        return Err("cannot start reload validation".into());
    }
    if pid == 0 {
        // Only async-signal-safe calls between fork and exec; driver threads and
        // the parent's allocator are never touched in this child.
        unsafe {
            // The validation process must neither consume nor retain a PTY,
            // listener, waiter, display or graphics-driver descriptor. Kernel
            // range closing covers FDs a driver thread opened near fork too.
            let mut first = 3;
            for &keep in &validation_fds {
                if first < keep && close_range(first, keep - 1, 0) < 0 {
                    os::_exit(127);
                }
                first = keep + 1;
            }
            if close_range(first, u32::MAX, 0) < 0 {
                os::_exit(127);
            }
            if os::fcntl(state.0, 2, 0) >= 0 && os::fcntl(ack_write.0, 2, 0) >= 0 {
                execveat(image.0, c"".as_ptr(), check.as_ptr(), env.as_ptr(), 0x1000);
            }
            os::_exit(127);
        }
    }
    drop(ack_write);
    let until = os::milliseconds().saturating_add(10_000);
    let mut status = 0;
    loop {
        let result = unsafe { os::waitpid(pid, &mut status, 1) };
        if result == pid {
            break;
        }
        if result < 0 && os::errno() != os::EINTR {
            return Err("reload validator could not be reaped".into());
        }
        if os::milliseconds() >= until {
            unsafe {
                os::kill(pid, 9);
            }
            loop {
                let waited = unsafe { os::waitpid(pid, &mut status, 0) };
                if waited == pid || (waited < 0 && os::errno() != os::EINTR) {
                    break;
                }
            }
            return Err(
                "new executable reload validation timed out; original sessions kept".into(),
            );
        }
        unsafe {
            os::poll(core::ptr::null_mut(), 0, 10);
        }
    }
    if status != 0 {
        return Err("new executable rejected the checkpoint; original sessions kept".into());
    }
    let mut acknowledged = [0u8; 41];
    let count = loop {
        let count = unsafe {
            os::read(
                ack_read.0,
                acknowledged.as_mut_ptr().cast(),
                acknowledged.len(),
            )
        };
        if count < 0 && os::errno() == os::EINTR {
            continue;
        }
        break count;
    };
    if count != 40 || acknowledged[..8] != VALIDATED[..] || &acknowledged[8..40] != nonce.to_bytes()
    {
        return Err(
            "new executable does not support validated runtime reload; original sessions kept"
                .into(),
        );
    }
    drop(ack_read);
    let flags = open_descriptors()?;
    if inherited
        .iter()
        .any(|fd| !flags.iter().any(|(open, _)| open == fd))
    {
        return Err("a reload descriptor disappeared before commit; original sessions kept".into());
    }
    let result = (|| {
        for &(fd, original) in &flags {
            let next = if fd == state.0 || inherited.contains(&fd) {
                original & !FD_CLOEXEC
            } else {
                original | FD_CLOEXEC
            };
            if unsafe { os::fcntl(fd, 2, next) } < 0 {
                return Err("cannot prepare reload descriptor inheritance".into());
            }
        }
        unsafe {
            execveat(image.0, c"".as_ptr(), resume.as_ptr(), env.as_ptr(), 0x1000);
        }
        Err(format!(
            "reload exec failed (OS error {}); original sessions kept",
            os::errno()
        ))
    })();
    for (fd, original) in flags {
        unsafe {
            os::fcntl(fd, 2, original);
        }
    }
    result
}
