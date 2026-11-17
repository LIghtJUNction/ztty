#![cfg(target_os = "linux")]

//! Real libc regressions for the production cwd guard. Cwd, directory modes
//! and the temporary FD limit are restored by RAII; tests serialize mutations.
extern crate alloc;

use std::collections::BTreeSet;
use std::ffi::{CString, c_int, c_ulong};
use std::fs::{self, File, Permissions};
use std::os::fd::AsRawFd;
use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

mod os {
    use core::ffi::{c_char, c_int};

    #[link(name = "c")]
    unsafe extern "C" {
        pub fn open(path: *const c_char, flags: c_int, ...) -> c_int;
        pub fn chdir(path: *const c_char) -> c_int;
        pub fn fchdir(fd: c_int) -> c_int;
        pub fn close(fd: c_int) -> c_int;
        pub fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
        pub fn geteuid() -> u32;
        fn __errno_location() -> *mut c_int;
    }

    pub fn errno() -> i32 {
        unsafe { *__errno_location() }
    }
}

mod broker {
    pub struct Fd(pub i32);

    impl Drop for Fd {
        fn drop(&mut self) {
            unsafe { super::os::close(self.0) };
        }
    }
}

#[path = "../src/working_directory.rs"]
mod working_directory;
use working_directory::WorkingDirectory;

static SERIAL: Mutex<()> = Mutex::new(());
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
const F_GETFD: c_int = 1;
const F_GETFL: c_int = 3;
const FD_CLOEXEC: c_int = 1;

struct Fixture {
    root: PathBuf,
    origin: PathBuf,
    target: PathBuf,
    saved: File,
    _serial: MutexGuard<'static, ()>,
}

impl Fixture {
    fn new() -> Self {
        let serial = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
        let saved = File::open(".").unwrap();
        let root = std::env::temp_dir().join(format!(
            "ztty-cwd-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let fixture = Self {
            origin: root.join("origin"),
            target: root.join("target"),
            root,
            saved,
            _serial: serial,
        };
        fs::set_permissions(&fixture.root, Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(&fixture.origin).unwrap();
        fs::create_dir(&fixture.target).unwrap();
        std::env::set_current_dir(&fixture.origin).unwrap();
        fixture
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Restore global cwd before permission cleanup, including on panic.
        unsafe { os::fchdir(self.saved.as_raw_fd()) };
        let _ = fs::set_permissions(&self.origin, Permissions::from_mode(0o700));
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn c_path(path: &Path) -> CString {
    CString::new(path.as_os_str().as_bytes()).unwrap()
}

fn open_fds() -> BTreeSet<c_int> {
    // read_dir's own FD closes when collection finishes. Exclude that closed
    // descriptor with real fcntl, without allocating another descriptor.
    let candidates: Vec<c_int> = fs::read_dir("/proc/self/fd")
        .unwrap()
        .map(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_str()
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect();
    candidates
        .into_iter()
        .filter(|&fd| unsafe { os::fcntl(fd, F_GETFD) } >= 0)
        .collect()
}

#[test]
fn switches_restores_and_closes_a_cloexec_path_handle() {
    let fixture = Fixture::new();
    let before = open_fds();
    let guard = WorkingDirectory::enter(&c_path(&fixture.target)).unwrap();
    assert_eq!(std::env::current_dir().unwrap(), fixture.target);
    let after = open_fds();
    let added: Vec<_> = after.difference(&before).copied().collect();
    assert_eq!(added.len(), 1, "one saved-directory handle is retained");
    let fd = added[0];
    assert_eq!(unsafe { os::fcntl(fd, F_GETFD) } & FD_CLOEXEC, FD_CLOEXEC);
    assert_ne!(unsafe { os::fcntl(fd, F_GETFL) } & (1 << 21), 0);
    assert_eq!(
        fs::read_link(format!("/proc/self/fd/{fd}")).unwrap(),
        fixture.origin
    );
    drop(guard);
    assert_eq!(std::env::current_dir().unwrap(), fixture.origin);
    assert_eq!(open_fds(), before);
}

#[test]
fn search_only_original_directory_can_be_saved_and_restored() {
    let fixture = Fixture::new();
    assert_ne!(
        unsafe { os::geteuid() },
        0,
        "permission regression must run as a non-root user"
    );
    fs::set_permissions(&fixture.origin, Permissions::from_mode(0o100)).unwrap();
    assert_eq!(
        fs::read_dir(&fixture.origin).unwrap_err().raw_os_error(),
        Some(13)
    );
    let before = open_fds();
    let guard = WorkingDirectory::enter(&c_path(&fixture.target)).unwrap();
    assert_eq!(std::env::current_dir().unwrap(), fixture.target);
    drop(guard);
    assert_eq!(std::env::current_dir().unwrap(), fixture.origin);
    assert_eq!(open_fds(), before);
}

#[test]
fn failed_target_preserves_cwd_errno_and_descriptor_count() {
    let fixture = Fixture::new();
    let file = fixture.root.join("regular-file");
    fs::write(&file, b"not a directory").unwrap();
    let before = open_fds();
    for (target, errno) in [(fixture.root.join("missing"), 2), (file, 20)] {
        for _ in 0..16 {
            let error = WorkingDirectory::enter(&c_path(&target))
                .err()
                .expect("target must fail");
            assert_eq!(
                error,
                format!("cannot enter session working directory (OS error {errno})")
            );
            assert_eq!(std::env::current_dir().unwrap(), fixture.origin);
        }
    }
    assert_eq!(open_fds(), before);
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Limit {
    current: c_ulong,
    maximum: c_ulong,
}

#[link(name = "c")]
unsafe extern "C" {
    fn getrlimit(resource: c_int, limit: *mut Limit) -> c_int;
    fn setrlimit(resource: c_int, limit: *const Limit) -> c_int;
}

struct FileLimit(Limit);
impl FileLimit {
    fn lower() -> Self {
        let mut old = Limit {
            current: 0,
            maximum: 0,
        };
        assert_eq!(unsafe { getrlimit(7, &mut old) }, 0); // RLIMIT_NOFILE
        let limit = Self(old);
        let reduced = Limit {
            current: old.current.min(64),
            ..old
        };
        assert_eq!(unsafe { setrlimit(7, &reduced) }, 0);
        limit
    }
}
impl Drop for FileLimit {
    fn drop(&mut self) {
        unsafe { setrlimit(7, &self.0) };
    }
}

#[test]
fn saved_directory_failure_reports_original_emfile_before_target_lookup() {
    let fixture = Fixture::new();
    let before = open_fds();
    let limit = FileLimit::lower();
    let mut occupied = Vec::new();
    loop {
        match File::open("/dev/null") {
            Ok(file) => occupied.push(file),
            Err(error) => {
                assert_eq!(error.raw_os_error(), Some(24)); // EMFILE
                break;
            }
        }
    }
    let error = WorkingDirectory::enter(&c_path(&fixture.root.join("missing")))
        .err()
        .expect("saving cwd must fail with the exhausted real FD limit");
    drop(occupied);
    drop(limit);
    assert_eq!(error, "cannot save session working directory (OS error 24)");
    assert_eq!(std::env::current_dir().unwrap(), fixture.origin);
    assert_eq!(open_fds(), before);
}
