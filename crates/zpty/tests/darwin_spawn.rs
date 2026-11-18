// This executable test owns its complete child tree. It must run on real macOS;
// cross-target compilation and the non-Darwin skip do not constitute acceptance.
#[cfg(not(target_os = "macos"))]
fn main() {
    println!("SKIP darwin_spawn: requires native macOS");
}

#[cfg(target_os = "macos")]
fn main() {
    darwin::main();
}

#[cfg(target_os = "macos")]
mod darwin {
    use std::ffi::{CStr, CString};
    use std::fs::{self, File, OpenOptions};
    use std::io::Write;
    use std::ops::{Deref, DerefMut};
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    use zpty::{Pty, Size};

    unsafe extern "C" {
        fn fcntl(fd: i32, command: i32, ...) -> i32;
        fn close(fd: i32) -> i32;
        fn getpid() -> i32;
        fn getsid(pid: i32) -> i32;
        fn getpgrp() -> i32;
        fn tcgetpgrp(fd: i32) -> i32;
        fn ioctl(fd: i32, request: u64, ...) -> i32;
        fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
        fn kill(pid: i32, signal: i32) -> i32;
        fn proc_listchildpids(pid: i32, buffer: *mut std::ffi::c_void, bytes: i32) -> i32;
        fn proc_pidpath(pid: i32, buffer: *mut std::ffi::c_void, bytes: u32) -> i32;
        fn __error() -> *mut i32;
        fn write(fd: i32, bytes: *const std::ffi::c_void, length: usize) -> isize;
    }

    static CLEANUP_FAILED: AtomicBool = AtomicBool::new(false);

    fn diagnostic(message: std::fmt::Arguments<'_>) {
        // Closed-stdio subtests intentionally close stderr. Diagnostic output
        // must not reopen that slot or panic when the write is unavailable.
        let _ = writeln!(std::io::stderr(), "{message}");
    }

    struct Child(Option<Pty>);

    impl Deref for Child {
        type Target = Pty;

        fn deref(&self) -> &Self::Target {
            self.0.as_ref().unwrap()
        }
    }

    impl DerefMut for Child {
        fn deref_mut(&mut self) -> &mut Self::Target {
            self.0.as_mut().unwrap()
        }
    }

    impl Drop for Child {
        fn drop(&mut self) {
            let Some(mut pty) = self.0.take() else {
                return;
            };
            if pty.status().is_some() {
                return;
            }
            let pid = pty.pid();
            let _ = pty.hangup();
            let deadline = Instant::now() + Duration::from_millis(200);
            let mut buffer = [0_u8; 4096];
            loop {
                let _ = pty.read(&mut buffer);
                if pty.wait_timeout(0).ok().flatten().is_some() {
                    return;
                }
                if Instant::now() >= deadline {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
            unsafe { kill(pid, 9) };
            // Closing our master also releases any slave-close output drain.
            // Pty::drop may reap already, so ECHILD below is an owned success.
            drop(pty);
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let mut status = 0;
                let result = unsafe { waitpid(pid, &mut status, 1) };
                let errno = if result < 0 { unsafe { *__error() } } else { 0 };
                if result == pid || result == -1 && errno == 10 {
                    return;
                }
                if result < 0 && errno != 4 || Instant::now() >= deadline {
                    CLEANUP_FAILED.store(true, Ordering::Release);
                    diagnostic(format_args!(
                        "darwin_spawn: cleanup failed for owned child pid={pid}: waitpid={result}, errno={errno}"
                    ));
                    return;
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
    }

    struct Workers {
        stop: Arc<AtomicBool>,
        ticks: Arc<AtomicU64>,
        handles: Vec<thread::JoinHandle<()>>,
    }

    impl Workers {
        fn start() -> Self {
            let stop = Arc::new(AtomicBool::new(false));
            let ticks = Arc::new(AtomicU64::new(0));
            let ready = Arc::new(Barrier::new(3));
            let handles = (0..2)
                .map(|_| {
                    let stop = stop.clone();
                    let ticks = ticks.clone();
                    let ready = ready.clone();
                    thread::spawn(move || {
                        ready.wait();
                        while !stop.load(Ordering::Acquire) {
                            let mut allocation = vec![17_u8; 8192];
                            allocation.push(31);
                            std::hint::black_box(allocation);
                            ticks.fetch_add(1, Ordering::Relaxed);
                            thread::sleep(Duration::from_millis(1));
                        }
                    })
                })
                .collect();
            ready.wait();
            while ticks.load(Ordering::Acquire) == 0 {
                thread::yield_now();
            }
            Self {
                stop,
                ticks,
                handles,
            }
        }
    }

    impl Drop for Workers {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            for handle in self.handles.drain(..) {
                handle.join().unwrap();
            }
        }
    }

    struct Directory(PathBuf);

    impl Directory {
        fn new() -> Self {
            let suffix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("zpty-spawn-{}-{suffix}", std::process::id()));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn c_path(path: &Path) -> CString {
        CString::new(path.as_os_str().as_bytes()).unwrap()
    }

    fn open_fds() -> Vec<(i32, i32)> {
        (0..256)
            .filter_map(|fd| {
                let flags = unsafe { fcntl(fd, 1) };
                (flags >= 0).then_some((fd, flags))
            })
            .collect()
    }

    fn child_diagnostics(context: &str) {
        let mut children = [0_i32; 32];
        let count = unsafe {
            proc_listchildpids(
                getpid(),
                children.as_mut_ptr().cast(),
                std::mem::size_of_val(&children) as i32,
            )
        };
        diagnostic(format_args!(
            "darwin_spawn: {context}: owned child inventory count={count}, capacity={}",
            children.len()
        ));
        let count = usize::try_from(count).unwrap_or(0).min(children.len());
        for &pid in &children[..count] {
            if pid <= 0 {
                continue;
            }
            let mut path = [0_u8; 4096];
            let length = unsafe { proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
            let path = if length > 0 {
                CStr::from_bytes_until_nul(&path).ok()
            } else {
                None
            };
            diagnostic(format_args!(
                "darwin_spawn: {context}: owned child pid={pid}, executable={path:?}, path_result={length}"
            ));
        }
    }

    fn no_children(context: &str) {
        // This dedicated test executable has no foreign children. All its
        // compiler, std::process and PTY children have already been joined.
        let mut status = 0;
        let result = unsafe { waitpid(-1, &mut status, 1) };
        let errno = if result < 0 { unsafe { *__error() } } else { 0 };
        if result != -1 || errno != 10 {
            child_diagnostics(context);
        }
        assert_eq!(
            result, -1,
            "{context}: unreaped owned child, returned_pid={result}, status={status}, errno={errno}"
        );
        assert_eq!(errno, 10, "{context}: expected ECHILD");
    }

    fn collect_until(pty: &Pty, marker: &[u8], out: &mut Vec<u8>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            // READY and DONE can arrive in the same read. Retain both rather
            // than waiting for a marker which has already been consumed.
            if out.windows(marker.len()).any(|part| part == marker) {
                return;
            }
            let mut buffer = [0_u8; 4096];
            match pty.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    out.extend_from_slice(&buffer[..count]);
                    assert!(out.len() <= 64 * 1024, "unbounded test output");
                    if out.windows(marker.len()).any(|part| part == marker) {
                        return;
                    }
                }
                Err(error) if error.is_would_block() => thread::sleep(Duration::from_millis(1)),
                Err(error) => panic!("PTY read: {error}"),
            }
        }
        panic!(
            "child pid={} missing {:?} in {:?}",
            pty.pid(),
            String::from_utf8_lossy(marker),
            String::from_utf8_lossy(out)
        );
    }

    fn finish_probe(child: &mut Child, out: &mut Vec<u8>) {
        // Waiting alone does not service the PTY. Keep consuming output while
        // checking the same two-second child-exit deadline, including EOF.
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let mut buffer = [0_u8; 4096];
            let eof = match child.read(&mut buffer) {
                Ok(0) => true,
                Ok(count) => {
                    out.extend_from_slice(&buffer[..count]);
                    assert!(out.len() <= 64 * 1024, "unbounded test output");
                    false
                }
                Err(error) if error.is_would_block() => false,
                Err(error) => panic!("child pid={} final PTY read: {error}", child.pid()),
            };
            let status = child.wait_timeout(0).unwrap();
            if let Some(status) = status
                && eof
            {
                assert!(
                    out.windows(b"ZPTY_DONE".len())
                        .any(|part| part == b"ZPTY_DONE"),
                    "child pid={} exited without DONE: output={:?}",
                    child.pid(),
                    String::from_utf8_lossy(out),
                );
                assert!(
                    status.success(),
                    "child pid={} status {status:?}",
                    child.pid()
                );
                diagnostic(format_args!(
                    "darwin_spawn: child pid={} phase=exited-and-drained",
                    child.pid()
                ));
                return;
            }
            assert!(
                Instant::now() < deadline,
                "child pid={} did not exit and drain within 2s: status={status:?}, eof={eof}, output={:?}",
                child.pid(),
                String::from_utf8_lossy(out),
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn send(pty: &Pty, mut bytes: &[u8]) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !bytes.is_empty() && Instant::now() < deadline {
            match pty.write(bytes) {
                Ok(0) => panic!("zero-length write"),
                Ok(count) => bytes = &bytes[count..],
                Err(error) if error.is_would_block() => thread::sleep(Duration::from_millis(1)),
                Err(error) => panic!("PTY write: {error}"),
            }
        }
        assert!(bytes.is_empty(), "PTY write timeout");
    }

    fn check_terminal(expected: Size) {
        let pid = unsafe { getpid() };
        assert_eq!(unsafe { getsid(0) }, pid);
        assert_eq!(unsafe { getpgrp() }, pid);
        for fd in [0, 1, 2] {
            assert_eq!(unsafe { tcgetpgrp(fd) }, pid, "foreground group on fd {fd}");
            assert_eq!(unsafe { fcntl(fd, 1) } & 1, 0, "stdio close-on-exec");
            let mut actual = Size::new(0, 0);
            assert_eq!(unsafe { ioctl(fd, 0x4008_7468, &mut actual) }, 0);
            assert_eq!(actual, expected);
        }
        let tty = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .unwrap();
        assert_eq!(
            unsafe { tcgetpgrp(tty.as_raw_fd()) },
            pid,
            "/dev/tty is not the session's foreground terminal"
        );
        drop(tty);
        assert!(
            open_fds().iter().all(|&(fd, _)| fd <= 2),
            "private inherited fd survived target exec"
        );
    }

    fn probe_child(arguments: &[CString]) {
        let mode = arguments
            .get(2)
            .map(|arg| arg.to_bytes())
            .unwrap_or_default();
        let mut expected = Size::new(80, 24);
        if mode == b"metadata" {
            expected.pixel_width = 640;
            expected.pixel_height = 480;
        }
        check_terminal(expected);
        if mode == b"metadata" {
            assert_eq!(arguments.len(), 5);
            assert_eq!(arguments[0].to_bytes(), b"deliberate argv zero");
            assert_eq!(arguments[3].to_bytes(), "space ' quote \" 雪".as_bytes());
            assert_eq!(arguments[4].to_bytes(), b"non-utf8-\xff");
            assert_eq!(
                std::env::var_os("ZPTY_VALUE").unwrap().as_bytes(),
                b"value with spaces\xff"
            );
            assert!(
                std::env::var_os("HOME").is_none(),
                "environment was implicitly inherited"
            );
            let expected = std::env::var_os("ZPTY_CWD").unwrap();
            assert_eq!(
                std::env::current_dir().unwrap().as_os_str(),
                expected.as_os_str()
            );
        }
        println!("ZPTY_READY");
        std::io::stdout().flush().unwrap();
        if mode == b"metadata" {
            let mut input = String::new();
            std::io::stdin().read_line(&mut input).unwrap();
            assert_eq!(input, "round trip\n");
            check_terminal(Size::new(100, 30));
        }
        println!("ZPTY_DONE");
    }

    fn probe(helper: &CStr, metadata: bool) {
        let extra = CString::new("space ' quote \" 雪").unwrap();
        let bytes = CString::new(b"non-utf8-\xff".as_slice()).unwrap();
        let value = CString::new(b"ZPTY_VALUE=value with spaces\xff".as_slice()).unwrap();
        let mut cwd = b"ZPTY_CWD=".to_vec();
        cwd.extend_from_slice(std::env::current_dir().unwrap().as_os_str().as_bytes());
        let cwd = CString::new(cwd).unwrap();
        let arguments = if metadata {
            vec![
                c"deliberate argv zero",
                c"--probe-child",
                c"metadata",
                &extra,
                &bytes,
            ]
        } else {
            vec![c"deliberate argv zero", c"--probe-child", c"simple"]
        };
        let mut size = Size::new(80, 24);
        if metadata {
            size.pixel_width = 640;
            size.pixel_height = 480;
        }
        let pty = Pty::spawn_with_helper(
            helper,
            helper,
            &arguments,
            &[c"PATH=/usr/bin:/bin", c"TERM=xterm-256color", &value, &cwd],
            size,
        )
        .unwrap();
        let mut child = Child(Some(pty));
        assert!(child.fd() > 2);
        assert_eq!(unsafe { fcntl(child.fd(), 1) } & 1, 1);
        assert_eq!(unsafe { fcntl(child.fd(), 3) } & 4, 4);
        diagnostic(format_args!(
            "darwin_spawn: child pid={} metadata={metadata} phase=spawned",
            child.pid()
        ));
        let mut output = Vec::new();
        collect_until(&child, b"ZPTY_READY", &mut output);
        if metadata {
            child.resize(Size::new(100, 30)).unwrap();
            send(&child, b"round trip\n");
            collect_until(&child, b"ZPTY_DONE", &mut output);
        }
        diagnostic(format_args!(
            "darwin_spawn: child pid={} phase=drain-and-exit deadline_ms=2000",
            child.pid()
        ));
        finish_probe(&mut child, &mut output);
    }

    fn expect_error(helper: &CStr, program: &CStr, environment: &[&CStr], errno: i32) {
        let context = format!(
            "spawn-error helper={helper:?}, program={program:?}, environment={environment:?}, expected_errno={errno}"
        );
        diagnostic(format_args!("darwin_spawn: phase=before {context}"));
        let before = open_fds();
        let failure = match Pty::spawn_with_helper(
            helper,
            program,
            &[c"probe"],
            environment,
            Size::new(80, 24),
        ) {
            Ok(pty) => {
                let _cleanup = Child(Some(pty));
                panic!("expected spawn error {errno}");
            }
            Err(error) => error,
        };
        diagnostic(format_args!(
            "darwin_spawn: phase=returned {context}, actual={failure:?}"
        ));
        assert_eq!(failure.operation, zpty::Operation::Spawn);
        assert_eq!(failure.errno, errno);
        assert_eq!(open_fds(), before, "failed spawn leaked descriptors");
        no_children(&context);
    }

    fn closed_stdio(arguments: &[CString]) {
        let mask: i32 = arguments[2].to_str().unwrap().parse().unwrap();
        let report_path = Path::new(std::ffi::OsStr::from_bytes(arguments[3].to_bytes()));
        let mut report = File::create(report_path).unwrap();
        for fd in [0, 1, 2] {
            if mask & (1 << fd) != 0 {
                assert_eq!(unsafe { close(fd) }, 0);
            }
        }
        let before = open_fds();
        let helper = zpty::current_executable().unwrap();
        probe(&helper, false);
        expect_error(&helper, c"/zpty-test-no-such-executable", &[], 2);
        assert_eq!(open_fds(), before);
        for fd in [0, 1, 2] {
            if mask & (1 << fd) != 0 {
                assert_eq!(
                    unsafe { fcntl(fd, 1) },
                    -1,
                    "parent stdio slot was left occupied"
                );
            }
        }
        writeln!(report, "closed-stdio-mask={mask}: passed").unwrap();
    }

    fn sdk_abi() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/darwin_spawn_abi.c");
        let output = Command::new("/usr/bin/cc")
            .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-fsyntax-only"])
            .arg(source)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Apple SDK ABI check: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    pub fn main() {
        let arguments: Vec<CString> = std::env::args_os()
            .map(|arg| CString::new(arg.as_bytes()).unwrap())
            .collect();
        let args: Vec<&CStr> = arguments.iter().map(CString::as_c_str).collect();
        // These deliberately broken helpers exist only in this test executable.
        // They test the public IPC boundary without production test hooks.
        if args.get(1).copied() == Some(c"--zpty-spawn-helper-v1") {
            match std::env::var("ZPTY_TEST_BROKEN_HELPER").as_deref() {
                Ok("exit") => std::process::exit(0),
                Ok("crash") => {
                    unsafe { kill(getpid(), 9) };
                    std::process::exit(127);
                }
                Ok("partial") => {
                    let fd: i32 = args[2].to_str().unwrap().parse().unwrap();
                    unsafe { write(fd, b"ZP".as_ptr().cast(), 2) };
                    std::process::exit(0);
                }
                Ok("stall") => {
                    thread::sleep(Duration::from_secs(30));
                    std::process::exit(127);
                }
                _ => {}
            }
        }
        zpty::run_spawn_helper(&args);
        if args.get(1).copied() == Some(c"--probe-child") {
            probe_child(&arguments);
            return;
        }
        if args.get(1).copied() == Some(c"--closed-stdio-subtest") {
            closed_stdio(&arguments);
            return;
        }

        let started = Instant::now();
        let directory = Directory::new();
        sdk_abi();
        let helper = zpty::current_executable().unwrap();
        let baseline = open_fds();
        let workers = Workers::start();
        let ticks_before = workers.ticks.load(Ordering::Relaxed);
        let mut checks = vec!["Apple SDK ABI constants, layout and prototypes"];

        let old_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&directory.0).unwrap();
        assert_eq!(zpty::current_executable().unwrap(), helper);
        probe(&helper, true);
        std::env::set_current_dir(old_cwd).unwrap();
        checks.push("worker-active spawn: controlling tty, session/group, exact argv/env/cwd, input and resize");

        let sentinel_path = directory.0.join("inherited-sentinel");
        let sentinel = File::create(&sentinel_path).unwrap();
        assert_eq!(unsafe { fcntl(sentinel.as_raw_fd(), 2, 0) }, 0);
        thread::scope(|scope| {
            let mut handles = Vec::new();
            for _ in 0..3 {
                let helper = &helper;
                handles.push(scope.spawn(move || {
                    for _ in 0..3 {
                        probe(helper, false);
                    }
                }));
            }
            for handle in handles {
                handle.join().unwrap();
            }
        });
        no_children("after nine concurrent probes");
        drop(sentinel);
        checks
            .push("nine concurrent PTY spawns drain READY/DONE through EOF, exit, and exclude peer handshakes and a non-CLOEXEC sentinel");

        let missing = c_path(&directory.0.join("missing-program"));
        expect_error(&helper, &missing, &[], 2);
        expect_error(&missing, c"/bin/sh", &[], 2);
        let denied = directory.0.join("not-executable");
        fs::write(&denied, b"not executable\n").unwrap();
        fs::set_permissions(&denied, fs::Permissions::from_mode(0o600)).unwrap();
        expect_error(&helper, &c_path(&denied), &[], 13);
        checks.push("real exec/helper errors preserve ENOENT/EACCES, close descriptors and reap");

        for mode in ["exit", "crash", "partial"] {
            let environment = CString::new(format!("ZPTY_TEST_BROKEN_HELPER={mode}")).unwrap();
            expect_error(&helper, c"/bin/true", &[&environment], 5);
        }
        checks.push(
            "empty/partial readiness and early helper death cannot report successful handoff",
        );
        let timeout_started = Instant::now();
        expect_error(
            &helper,
            c"/bin/true",
            &[c"ZPTY_TEST_BROKEN_HELPER=stall"],
            60,
        );
        let timeout_seconds = timeout_started.elapsed().as_secs_f64();
        assert!(
            (4.0..12.0).contains(&timeout_seconds),
            "handshake timeout took {timeout_seconds}s"
        );
        checks.push("stalled helper reaches the bounded handshake failure and is reaped");

        let pty = Pty::spawn_with_helper(
            &helper,
            c"/bin/sleep",
            &[c"sleep", c"30"],
            &[],
            Size::new(80, 24),
        )
        .unwrap();
        let mut child = Child(Some(pty));
        let master = child.fd();
        child.hangup().unwrap();
        let status = child
            .wait_timeout(2000)
            .unwrap()
            .expect("SIGHUP child did not exit");
        assert_eq!(status.signal(), Some(1));
        drop(child);
        assert_eq!(unsafe { fcntl(master, 1) }, -1);
        checks.push("hangup targets the owned session and wait/drop release its child and master");

        for mask in 1..=7 {
            let report = directory.0.join(format!("closed-stdio-{mask}.txt"));
            let status = Command::new(Path::new(std::ffi::OsStr::from_bytes(helper.to_bytes())))
                .arg0("deliberately-unresolvable-argv-zero")
                .arg("--closed-stdio-subtest")
                .arg(mask.to_string())
                .arg(&report)
                .current_dir(&directory.0)
                .status()
                .unwrap();
            assert!(
                status.success(),
                "closed stdio mask {mask} failed: {status}"
            );
            assert_eq!(
                fs::read_to_string(report).unwrap(),
                format!("closed-stdio-mask={mask}: passed\n")
            );
        }
        checks.push("seven isolated closed-stdio masks preserve control stdio and cannot collide with helper fd sources");
        let worker_iterations = workers.ticks.load(Ordering::Relaxed) - ticks_before;
        assert!(worker_iterations > 0);
        drop(workers);
        assert!(
            !CLEANUP_FAILED.load(Ordering::Acquire),
            "owned child cleanup failed"
        );
        assert_eq!(open_fds(), baseline, "suite leaked descriptors");
        no_children("suite complete");
        let encoded = checks
            .iter()
            .map(|check| format!("{check:?}"))
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"suite\":\"darwin-threaded-spawn\",\"passed\":true,\"checks\":[{encoded}],\"worker_iterations\":{worker_iterations},\"timeout_seconds\":{timeout_seconds},\"duration_seconds\":{},\"architecture\":{:?}}}",
            started.elapsed().as_secs_f64(),
            std::env::consts::ARCH
        );
    }
}
