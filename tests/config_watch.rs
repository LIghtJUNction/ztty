#![cfg(target_os = "linux")]

//! Real inotify regressions. The interposer only schedules filesystem changes;
//! every watch operation still executes through the actual libc implementation.
extern crate alloc;

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

mod os {
    use core::ffi::{c_int, c_void};

    #[link(name = "c")]
    unsafe extern "C" {
        pub fn read(fd: c_int, buffer: *mut c_void, count: usize) -> isize;
        pub fn close(fd: c_int) -> c_int;
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

#[path = "../src/config_watch.rs"]
mod config_watch;

#[link(name = "dl")]
unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

type AddWatch = unsafe extern "C" fn(c_int, *const c_char, u32) -> c_int;
static REAL_ADD_WATCH: OnceLock<AddWatch> = OnceLock::new();
static SERIAL: Mutex<()> = Mutex::new(());
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
static RACE: Mutex<Option<Race>> = Mutex::new(None);

struct Race {
    fd: i32,
    missing_parent: PathBuf,
    ancestor: PathBuf,
    create: PathBuf,
    saw_missing_parent: bool,
    fired: bool,
    error: Option<String>,
    events: Vec<String>,
}

/// Schedule mkdir strictly after the failed direct watch and before libc adds
/// the ancestor watch. No sleeps, fake events, or production hooks are used.
///
/// # Safety
/// The caller supplies the same valid C path required by libc inotify_add_watch.
#[unsafe(no_mangle)]
unsafe extern "C" fn inotify_add_watch(fd: c_int, path: *const c_char, mask: u32) -> c_int {
    let Some(real) = REAL_ADD_WATCH.get() else {
        return -1;
    };
    let bytes = unsafe { CStr::from_ptr(path) }.to_bytes();
    {
        let mut state = RACE.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(race) = state.as_mut()
            && race.fd == fd
            && bytes == race.ancestor.as_os_str().as_bytes()
            && race.saw_missing_parent
            && !race.fired
        {
            race.fired = true;
            race.events.push(format!(
                "mkdir {} before ancestor add-watch",
                race.create.display()
            ));
            if let Err(error) = fs::create_dir_all(&race.create) {
                race.error = Some(error.to_string());
            }
        }
    }
    let result = unsafe { real(fd, path, mask) };
    let mut state = RACE.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(race) = state.as_mut()
        && race.fd == fd
    {
        if result < 0 && bytes == race.missing_parent.as_os_str().as_bytes() {
            race.saw_missing_parent = true;
        }
        race.events.push(format!(
            "add-watch {} -> {result}",
            String::from_utf8_lossy(bytes)
        ));
    }
    result
}

struct Directory {
    path: PathBuf,
    _serial: MutexGuard<'static, ()>,
}

impl Directory {
    fn new() -> Self {
        let serial = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
        REAL_ADD_WATCH.get_or_init(|| {
            let symbol = unsafe { dlsym((-1isize) as *mut c_void, c"inotify_add_watch".as_ptr()) };
            assert!(!symbol.is_null(), "real libc inotify_add_watch is required");
            unsafe { core::mem::transmute::<*mut c_void, AddWatch>(symbol) }
        });
        let index = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ztty-config-watch-{}-{index}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        Self {
            path,
            _serial: serial,
        }
    }

    fn arm(&self, watcher: &config_watch::Watcher, parent: &Path) {
        *RACE.lock().unwrap_or_else(|error| error.into_inner()) = Some(Race {
            fd: watcher.fd(),
            missing_parent: parent.into(),
            ancestor: self.path.clone(),
            create: parent.into(),
            saw_missing_parent: false,
            fired: false,
            error: None,
            events: Vec::new(),
        });
    }

    fn assert_race_fired(&self) {
        let state = RACE.lock().unwrap_or_else(|error| error.into_inner());
        let race = state.as_ref().unwrap();
        eprintln!("inotify race trace:\n{}", race.events.join("\n"));
        assert!(
            race.saw_missing_parent,
            "direct parent must really be missing"
        );
        assert!(race.fired, "mkdir must precede the real ancestor add-watch");
        assert!(race.error.is_none(), "mkdir failed: {:?}", race.error);
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        *RACE.lock().unwrap_or_else(|error| error.into_inner()) = None;
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn c_path(path: &Path) -> CString {
    CString::new(path.as_os_str().as_bytes()).unwrap()
}

fn write_and_expect(watcher: &mut config_watch::Watcher, path: &Path, contents: &[u8]) {
    fs::write(path, contents).unwrap();
    assert!(
        watcher.changed().contains(&c_path(path)),
        "file close-write must reach the config watcher: {}",
        path.display()
    );
}

#[test]
fn missing_deep_directories_created_before_ancestor_watch_do_not_lose_file_updates() {
    let directory = Directory::new();
    let parent = directory.path.join("missing/deep/settings");
    let config = parent.join("ztty.conf");
    let mut watcher = config_watch::Watcher::new();
    assert!(watcher.fd() >= 0);
    directory.arm(&watcher, &parent);
    watcher.watch(&c_path(&config));
    directory.assert_race_fired();
    assert!(
        !config.exists(),
        "the file is deliberately written only after watch returns"
    );
    let _ = watcher.changed();
    write_and_expect(&mut watcher, &config, b"cursor-blink = false\n");
    write_and_expect(&mut watcher, &config, b"cursor-blink = true\n");
}

#[test]
fn renamed_parent_recreated_before_ancestor_watch_still_reloads_later_file() {
    let directory = Directory::new();
    let parent = directory.path.join("settings");
    let config = parent.join("ztty.conf");
    fs::create_dir(&parent).unwrap();
    fs::write(&config, b"cursor-blink = false\n").unwrap();
    let mut watcher = config_watch::Watcher::new();
    assert!(watcher.fd() >= 0);
    watcher.watch(&c_path(&config));
    assert!(watcher.changed().is_empty());
    fs::rename(&parent, directory.path.join("previous-settings")).unwrap();
    directory.arm(&watcher, &parent);
    assert!(watcher.changed().contains(&c_path(&config)));
    directory.assert_race_fired();
    assert!(
        !config.exists(),
        "replacement directory is still empty during reconnect"
    );
    let _ = watcher.changed();
    write_and_expect(&mut watcher, &config, b"cursor-blink = true\n");
    write_and_expect(&mut watcher, &config, b"cursor-blink = false\n");
}

#[test]
fn retaining_one_path_preserves_shared_descriptor_and_releases_the_last_reference() {
    let directory = Directory::new();
    let keep = directory.path.join("keep.conf");
    let remove = directory.path.join("remove.conf");
    let mut watcher = config_watch::Watcher::new();
    assert!(watcher.fd() >= 0);
    watcher.watch(&c_path(&keep));
    watcher.watch(&c_path(&remove));
    let count = |watcher: &config_watch::Watcher| {
        fs::read_to_string(format!("/proc/self/fdinfo/{}", watcher.fd()))
            .unwrap()
            .lines()
            .filter(|line| line.starts_with("inotify wd:"))
            .count()
    };
    assert_eq!(
        count(&watcher),
        1,
        "same directory must share a kernel watch"
    );
    watcher.retain(|path| path == c_path(&keep).as_c_str());
    assert_eq!(
        count(&watcher),
        1,
        "removing one config must keep its peer's watch"
    );
    fs::write(&remove, b"ignored = true\n").unwrap();
    assert!(watcher.changed().is_empty());
    write_and_expect(&mut watcher, &keep, b"cursor-blink = true\n");
    watcher.retain(|_| false);
    assert_eq!(
        count(&watcher),
        0,
        "last reference must release the kernel watch"
    );
    fs::write(&keep, b"cursor-blink = false\n").unwrap();
    assert!(watcher.changed().is_empty());
}
