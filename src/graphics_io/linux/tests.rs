extern crate std;
use super::*;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(alloc::format!(
            "ztty-graphics-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, data: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, data).unwrap();
        path
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn request(medium: Medium, name: &[u8], offset: u32, size: u32, limit: usize) -> ExternalRead<'_> {
    ExternalRead {
        medium,
        name,
        offset,
        size,
        limit,
    }
}
fn path_bytes(path: &std::path::Path) -> &[u8] {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes()
}
#[test]
fn regular_files_follow_symlinks_and_respect_byte_ranges() {
    let dir = Directory::new();
    let data = dir.file("pixels", &[9, 1, 2, 3, 4, 8]);
    let alias = dir.0.join("alias");
    symlink(&data, &alias).unwrap();
    assert_eq!(
        read(request(Medium::File, path_bytes(&alias), 1, 4, 4)).unwrap(),
        [1, 2, 3, 4]
    );
    assert!(data.exists() && alias.exists());
    assert!(read(request(Medium::File, path_bytes(&alias), 1, 5, 4)).is_err());
    assert!(read(request(Medium::File, path_bytes(&alias), u32::MAX, 4, 4)).is_err());
    assert!(read(request(Medium::File, path_bytes(&alias), 4, 4, 4)).is_err());
    assert_eq!(
        read(request(Medium::File, path_bytes(&alias), 2, 0, 4)).unwrap(),
        [2, 3, 4, 8]
    );
}
#[test]
fn devices_fifos_directories_and_pseudo_files_are_rejected_without_io() {
    let dir = Directory::new();
    let fifo = dir.0.join("pipe");
    let name = CString::new(path_bytes(&fifo)).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    for path in [
        path_bytes(&fifo),
        path_bytes(&dir.0),
        b"/dev/zero",
        b"/proc/self/mem",
        b"/sys/kernel/uevent_seqnum",
    ] {
        assert!(read(request(Medium::File, path, 0, 4, 4)).is_err());
    }
    let alias = dir.0.join("device-link");
    symlink("/dev/null", &alias).unwrap();
    assert!(read(request(Medium::File, path_bytes(&alias), 0, 4, 4)).is_err());
}
#[test]
fn temporary_cleanup_requires_private_parent_identity_and_protocol_name() {
    let dir = Directory::new();
    let eligible = dir.file("tty-graphics-protocol-data", &[1, 2, 3, 4]);
    assert_eq!(
        read(request(
            Medium::TemporaryFile,
            path_bytes(&eligible),
            0,
            0,
            4
        ))
        .unwrap(),
        [1, 2, 3, 4]
    );
    assert!(!eligible.exists());
    let keep = dir.file("not-a-protocol-file", &[1]);
    read(request(Medium::TemporaryFile, path_bytes(&keep), 0, 0, 4)).unwrap();
    assert!(keep.exists());
    let target = dir.file("tty-graphics-protocol-original", &[2]);
    let alias = dir.0.join("tty-graphics-protocol-link");
    symlink(&target, &alias).unwrap();
    read(request(Medium::TemporaryFile, path_bytes(&alias), 0, 0, 4)).unwrap();
    assert!(target.exists() && alias.exists());
    fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o755)).unwrap();
    read(request(Medium::TemporaryFile, path_bytes(&target), 0, 0, 4)).unwrap();
    assert!(target.exists());
}
#[test]
fn failed_read_does_not_delete_temporary_source() {
    let dir = Directory::new();
    let path = dir.file("tty-graphics-protocol-short", &[1, 2]);
    assert!(read(request(Medium::TemporaryFile, path_bytes(&path), 0, 4, 4)).is_err());
    assert!(path.exists());
}
#[test]
fn posix_shared_memory_reads_range_then_unlinks_owned_object() {
    let suffix = alloc::format!(
        "ztty-test-graphics-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    for slash in [false, true] {
        let name = if slash {
            alloc::format!("/{suffix}")
        } else {
            suffix.clone()
        };
        let c = CString::new(name.as_bytes()).unwrap();
        let fd = unsafe {
            libc::shm_open(
                c.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR | libc::O_CLOEXEC,
                0o600,
            )
        };
        assert!(fd >= 0);
        let fd = Fd(fd);
        assert_eq!(
            unsafe { libc::write(fd.0, [9u8, 1, 2, 3, 4, 8].as_ptr().cast(), 6) },
            6
        );
        let result = read(request(Medium::SharedMemory, name.as_bytes(), 1, 4, 4));
        let still_exists = std::path::Path::new("/dev/shm").join(&suffix).exists();
        unsafe {
            libc::shm_unlink(c.as_ptr());
        }
        assert_eq!(result.unwrap(), [1, 2, 3, 4]);
        assert!(!still_exists);
    }
}
#[test]
fn shared_memory_cannot_follow_links_or_escape_the_namespace() {
    let dir = Directory::new();
    let file = dir.file("target", &[1, 2, 3, 4]);
    let name = alloc::format!(
        "ztty-test-graphics-link-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let path = std::path::Path::new("/dev/shm").join(&name);
    symlink(&file, &path).unwrap();
    let result = read(request(Medium::SharedMemory, name.as_bytes(), 0, 4, 4));
    let retained = path.symlink_metadata().is_ok();
    fs::remove_file(&path).unwrap();
    assert!(result.is_err() && retained && file.exists());
    for name in [b"../x".as_slice(), b"//x", b"/x/y", b".", b""] {
        assert!(read(request(Medium::SharedMemory, name, 0, 4, 4)).is_err());
    }
}

#[test]
fn normal_root_owned_sticky_temp_path_is_cleaned_without_leaking_files() {
    use std::io::Write;
    let path = std::path::Path::new("/tmp").join(alloc::format!(
        "tty-graphics-protocol-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    file.write_all(&[1, 2, 3, 4]).unwrap();
    drop(file);
    let result = read(request(Medium::TemporaryFile, path_bytes(&path), 0, 0, 4));
    let retained = path.exists();
    let _ = fs::remove_file(&path);
    assert_eq!(result.unwrap(), [1, 2, 3, 4]);
    assert!(!retained);
}
