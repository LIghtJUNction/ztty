use super::*;
use alloc::{ffi::CString, format};
use core::{ffi::CStr, mem::MaybeUninit};
use zterm::graphics::Medium;

const MAX_READ: usize = 16 * 1024 * 1024;

struct Fd(i32);
impl Drop for Fd {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.0);
        }
    }
}
fn open(path: &CStr, flags: i32) -> Result<Fd, ReadError> {
    let fd = unsafe { libc::open(path.as_ptr(), flags) };
    if fd < 0 { Err(ReadError) } else { Ok(Fd(fd)) }
}
fn stat(fd: &Fd) -> Result<libc::stat, ReadError> {
    let mut value = MaybeUninit::uninit();
    if unsafe { libc::fstat(fd.0, value.as_mut_ptr()) } != 0 {
        return Err(ReadError);
    }
    Ok(unsafe { value.assume_init() })
}
fn same(a: &libc::stat, b: &libc::stat) -> bool {
    a.st_dev == b.st_dev && a.st_ino == b.st_ino
}
fn path_of(fd: &Fd) -> Result<Vec<u8>, ReadError> {
    let name = CString::new(format!("/proc/self/fd/{}", fd.0)).map_err(|_| ReadError)?;
    let mut bytes = alloc::vec![0u8;4097];
    let n = unsafe { libc::readlink(name.as_ptr(), bytes.as_mut_ptr().cast(), bytes.len()) };
    if n < 0 || n as usize >= bytes.len() {
        return Err(ReadError);
    }
    bytes.truncate(n as usize);
    Ok(bytes)
}
fn sensitive(path: &[u8]) -> bool {
    path == b"/proc"
        || path.starts_with(b"/proc/")
        || path == b"/sys"
        || path.starts_with(b"/sys/")
        || (path == b"/dev" || path.starts_with(b"/dev/")) && !path.starts_with(b"/dev/shm/")
}
fn read_regular(
    path: &CStr,
    request: ExternalRead<'_>,
) -> Result<(Vec<u8>, Fd, libc::stat, Vec<u8>), ReadError> {
    if sensitive(path.to_bytes()) {
        return Err(ReadError);
    }
    // O_PATH resolves symlinks without opening devices/FIFOs for I/O.
    // Reopen the pinned inode only after type/size checks, never the pathname.
    let flags = libc::O_PATH
        | libc::O_CLOEXEC
        | if request.medium == Medium::SharedMemory {
            libc::O_NOFOLLOW
        } else {
            0
        };
    let anchor = open(path, flags)?;
    let metadata = stat(&anchor)?;
    let resolved = path_of(&anchor)?;
    if metadata.st_mode & libc::S_IFMT != libc::S_IFREG
        || metadata.st_size < 0
        || sensitive(&resolved)
    {
        return Err(ReadError);
    }
    let available = (metadata.st_size as u64)
        .checked_sub(u64::from(request.offset))
        .ok_or(ReadError)?;
    let size = if request.size == 0 {
        available
    } else {
        u64::from(request.size)
    };
    if size > available || size > request.limit.min(MAX_READ) as u64 {
        return Err(ReadError);
    }
    let descriptor = CString::new(format!("/proc/self/fd/{}", anchor.0)).map_err(|_| ReadError)?;
    let file = open(
        &descriptor,
        libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NONBLOCK,
    )?;
    let opened = stat(&file)?;
    if !same(&metadata, &opened) || opened.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(ReadError);
    }
    let size = size as usize;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(size).map_err(|_| ReadError)?;
    bytes.resize(size, 0);
    let mut at = 0;
    let mut interrupted = 0;
    while at < size {
        let count = (size - at).min(65536);
        let n = unsafe {
            libc::pread(
                file.0,
                bytes[at..].as_mut_ptr().cast(),
                count,
                (u64::from(request.offset) + at as u64) as libc::off_t,
            )
        };
        if n < 0 && unsafe { *libc::__errno_location() } == libc::EINTR && interrupted < 32 {
            interrupted += 1;
            continue;
        }
        if n <= 0 {
            return Err(ReadError);
        }
        at += n as usize;
    }
    Ok((bytes, anchor, metadata, resolved))
}
fn private_temp_parent(path: &CStr, metadata: &libc::stat) -> Option<(Fd, CString)> {
    let bytes = path.to_bytes();
    let split = bytes.iter().rposition(|b| *b == b'/')?;
    let parent = CString::new(&bytes[..split]).ok()?;
    let leaf = CString::new(&bytes[split + 1..]).ok()?;
    if !leaf
        .to_bytes()
        .windows(b"tty-graphics-protocol".len())
        .any(|p| p == b"tty-graphics-protocol")
    {
        return None;
    }
    let known = bytes.starts_with(b"/tmp/")
        || bytes.starts_with(b"/var/tmp/")
        || bytes.starts_with(b"/dev/shm/");
    let configured = unsafe { libc::getenv(c"TMPDIR".as_ptr()) };
    let configured = if configured.is_null() {
        false
    } else {
        let prefix = unsafe { CStr::from_ptr(configured) }.to_bytes();
        prefix.starts_with(b"/")
            && bytes.starts_with(prefix)
            && bytes.get(prefix.len()) == Some(&b'/')
    };
    if !known && !configured {
        return None;
    }
    let directory = open(
        &parent,
        libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
    )
    .ok()?;
    let dir = stat(&directory).ok()?;
    let uid = unsafe { libc::geteuid() };
    // A private owner directory or root-owned sticky temp directory prevents
    // other users from replacing this user's entry between identity and unlink.
    // Same-UID programs already share filesystem authority; this is not a sandbox.
    let protected = (dir.st_uid == uid && dir.st_mode & 0o077 == 0)
        || (dir.st_uid == 0 && dir.st_mode & libc::S_ISVTX != 0);
    if !protected || metadata.st_uid != uid || metadata.st_nlink != 1 {
        return None;
    }
    let mut named = MaybeUninit::uninit();
    if unsafe {
        libc::fstatat(
            directory.0,
            leaf.as_ptr(),
            named.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return None;
    }
    let named = unsafe { named.assume_init() };
    if !same(metadata, &named) || named.st_mode & libc::S_IFMT != libc::S_IFREG {
        return None;
    }
    Some((directory, leaf))
}
pub(super) fn read(request: ExternalRead<'_>) -> Result<Vec<u8>, ReadError> {
    let original = CString::new(request.name).map_err(|_| ReadError)?;
    let path = if request.medium == Medium::SharedMemory {
        let name = request.name.strip_prefix(b"/").unwrap_or(request.name);
        if name.is_empty()
            || name.len() > 255
            || name.contains(&b'/')
            || name == b"."
            || name == b".."
        {
            return Err(ReadError);
        }
        let mut path = b"/dev/shm/".to_vec();
        path.extend_from_slice(name);
        CString::new(path).map_err(|_| ReadError)?
    } else {
        if !request.name.starts_with(b"/") {
            return Err(ReadError);
        }
        original.clone()
    };
    let (bytes, _anchor, metadata, resolved) = read_regular(&path, request)?;
    if request.medium == Medium::TemporaryFile
        && path.to_bytes() == resolved
        && let Some((directory, leaf)) = private_temp_parent(&path, &metadata)
    {
        unsafe {
            libc::unlinkat(directory.0, leaf.as_ptr(), 0);
        }
    }
    if request.medium == Medium::SharedMemory && metadata.st_uid == unsafe { libc::geteuid() } {
        // POSIX names are passed unmodified to unlink, including an optional
        // leading slash. Never unlink another user's readable shared object.
        if let Ok(current) = open(&path, libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            && let Ok(named) = stat(&current)
            && same(&metadata, &named)
        {
            unsafe {
                libc::shm_unlink(original.as_ptr());
            }
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
