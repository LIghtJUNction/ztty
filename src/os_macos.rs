//! Small Darwin libSystem boundary. Production Rust does not link `std`.
use alloc::ffi::CString;
use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::ffi::{CStr, c_char, c_int, c_long, c_void};

pub const EINTR: i32 = 4;

#[repr(C)]
struct TimeSpec {
    seconds: c_long,
    nanoseconds: c_long,
}

#[link(name = "c")]
unsafe extern "C" {
    pub fn write(fd: c_int, data: *const c_void, size: usize) -> isize;
    pub fn close(fd: c_int) -> c_int;
    fn open(path: *const c_char, flags: c_int, ...) -> c_int;
    fn read(fd: c_int, data: *mut c_void, size: usize) -> isize;
    pub fn getenv(name: *const c_char) -> *mut c_char;
    fn access(path: *const c_char, mode: c_int) -> c_int;
    fn _NSGetEnviron() -> *mut *mut *mut c_char;
    pub fn _exit(status: c_int) -> !;
    pub fn __error() -> *mut c_int;
    fn clock_gettime(clock: c_int, time: *mut TimeSpec) -> c_int;
    fn malloc(size: usize) -> *mut c_void;
    fn realloc(ptr: *mut c_void, size: usize) -> *mut c_void;
    fn free(ptr: *mut c_void);
    fn posix_memalign(ptr: *mut *mut c_void, alignment: usize, size: usize) -> c_int;
}

pub struct Allocator;
// libc allocations are process-wide and thread-safe; all Rust allocation layouts
// are honored, including alignments greater than malloc's native alignment.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.align() <= 16 {
            unsafe { malloc(layout.size().max(1)).cast() }
        } else {
            let mut p = core::ptr::null_mut();
            if unsafe { posix_memalign(&mut p, layout.align(), layout.size().max(1)) } == 0 {
                p.cast()
            } else {
                core::ptr::null_mut()
            }
        }
    }
    unsafe fn dealloc(&self, p: *mut u8, _: Layout) {
        unsafe { free(p.cast()) };
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if layout.align() <= 16 {
            return unsafe { realloc(p.cast(), size.max(1)).cast() };
        }
        let Ok(next) = Layout::from_size_align(size.max(1), layout.align()) else {
            return core::ptr::null_mut();
        };
        let out = unsafe { self.alloc(next) };
        if !out.is_null() {
            unsafe {
                core::ptr::copy_nonoverlapping(p, out, layout.size().min(size));
                self.dealloc(p, layout);
            }
        }
        out
    }
}

pub fn errno() -> i32 {
    unsafe { *__error() }
}
pub fn milliseconds() -> u64 {
    let mut t = TimeSpec {
        seconds: 0,
        nanoseconds: 0,
    };
    unsafe {
        clock_gettime(6, &mut t);
    }
    (t.seconds as u64) * 1000 + (t.nanoseconds as u64) / 1_000_000
}
pub fn print(fd: i32, bytes: &[u8]) {
    let mut remaining = bytes;
    while !remaining.is_empty() {
        let n = unsafe { write(fd, remaining.as_ptr().cast(), remaining.len()) };
        if n > 0 {
            remaining = &remaining[n as usize..];
        } else if n < 0 && errno() == EINTR {
            continue;
        } else {
            break;
        }
    }
}

pub fn env(name: &CStr) -> Option<&'static [u8]> {
    // This process never mutates its environment.
    let value = unsafe { getenv(name.as_ptr()) };
    (!value.is_null()).then(|| unsafe { CStr::from_ptr(value).to_bytes() })
}

pub fn read_config(path: &CStr) -> Result<Vec<u8>, i32> {
    const O_NONBLOCK: c_int = 0x0004;
    let fd = unsafe { open(path.as_ptr(), O_NONBLOCK) };
    if fd < 0 {
        return Err(errno());
    }
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    let result = loop {
        let size = unsafe { read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        match size {
            0 => break Ok(bytes),
            n if n < 0 && errno() == EINTR => continue,
            n if n < 0 => break Err(errno()),
            n if bytes.len() + n as usize > 65536 => break Err(27), // EFBIG
            n => bytes.extend_from_slice(&buffer[..n as usize]),
        }
    };
    unsafe { close(fd) };
    result
}

/// Copy the inherited environment before any fork. Environment entries stay in
/// their original byte encoding; UTF-8 is not imposed on Unix process metadata.
fn environment() -> Vec<CString> {
    let mut output = Vec::new();
    let mut pointer = unsafe { *_NSGetEnviron() };
    if !pointer.is_null() {
        unsafe {
            while !(*pointer).is_null() {
                output.push(CStr::from_ptr(*pointer).into());
                pointer = pointer.add(1);
            }
        }
    }
    output
}

fn resolve(program: &CStr, environment: &[CString]) -> Option<CString> {
    if program.to_bytes().contains(&b'/') {
        return Some(program.into());
    }
    let path = environment
        .iter()
        .find_map(|s| s.to_bytes().strip_prefix(b"PATH="))
        .unwrap_or(b"/usr/bin:/bin");
    for directory in path.split(|&b| b == b':') {
        let mut candidate = Vec::with_capacity(directory.len() + program.to_bytes().len() + 2);
        if directory.is_empty() {
            candidate.extend_from_slice(b".");
        } else {
            candidate.extend_from_slice(directory);
        }
        candidate.push(b'/');
        candidate.extend_from_slice(program.to_bytes());
        let path = CString::new(candidate).ok()?;
        if unsafe { access(path.as_ptr(), 1) } == 0 {
            return Some(path);
        }
    }
    None
}

/// Prepare every byte of the shell's environment and resolve PATH in the parent.
pub fn child_setup(program: &CStr) -> Result<(CString, Vec<CString>), &'static str> {
    let mut env = environment();
    let executable = resolve(program, &env).ok_or("command not found on PATH")?;
    env.retain(|entry| {
        ![
            b"TERM=".as_slice(),
            b"COLORTERM=",
            b"TERM_PROGRAM=",
            b"TERM_PROGRAM_VERSION=",
        ]
        .iter()
        .any(|prefix| entry.to_bytes().starts_with(prefix))
    });
    env.extend([
        c"TERM=xterm-256color".into(),
        c"COLORTERM=truecolor".into(),
        c"TERM_PROGRAM=ztty".into(),
        CString::new(concat!("TERM_PROGRAM_VERSION=", env!("CARGO_PKG_VERSION")))
            .expect("static version"),
    ]);
    Ok((executable, env))
}

/// Native macOS config location, with an explicit XDG override when set.
pub fn default_config_path() -> Option<CString> {
    let mut path;
    if let Some(base) = env(c"XDG_CONFIG_HOME").filter(|p| p.starts_with(b"/")) {
        path = base.to_vec();
    } else {
        path = env(c"HOME")?.to_vec();
        if !path.starts_with(b"/") {
            return None;
        }
        path.extend_from_slice(b"/Library/Application Support");
    }
    path.extend_from_slice(b"/ztty/ztty.conf");
    CString::new(path).ok()
}
