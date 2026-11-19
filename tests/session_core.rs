//! A single-threaded harness: libtest worker threads would violate forkpty's
//! process-wide safety contract. The PTY child is this executable, never a shell.
extern crate alloc;

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use zpty::Size;
use zterm::Terminal;
use ztty::{
    input_queue::InputQueue,
    session_control,
    session_core::{ReadStep, SessionCore},
};

const DEADLINE: Duration = Duration::from_secs(10);

struct Fixture {
    core: SessionCore,
    directory: PathBuf,
}

impl Fixture {
    fn new(mode: &str, limit: usize) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "ztty-session-core-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let executable = std::env::current_exe().unwrap();
        let arguments = [
            executable.as_os_str(),
            "--session-core-child".as_ref(),
            mode.as_ref(),
            directory.as_os_str(),
        ];
        #[cfg(unix)]
        let pty = {
            use std::{ffi::CString, os::unix::ffi::OsStrExt};
            let arguments: Vec<CString> = arguments
                .iter()
                .map(|argument| CString::new(argument.as_bytes()).unwrap())
                .collect();
            let argv: Vec<_> = arguments
                .iter()
                .map(|argument| argument.as_c_str())
                .collect();
            // This harness never creates threads or loads a UI framework.
            unsafe {
                zpty::Pty::spawn(
                    &arguments[0],
                    &argv,
                    &[c"TERM=xterm-256color"],
                    Size::new(40, 6),
                    &[],
                )
            }
            .unwrap()
        };
        #[cfg(windows)]
        let pty = {
            use std::os::windows::ffi::OsStrExt;
            let arguments: Vec<Vec<u16>> = arguments
                .iter()
                .map(|argument| argument.encode_wide().collect())
                .collect();
            let argv: Vec<_> = arguments.iter().map(Vec::as_slice).collect();
            zpty::Pty::spawn_utf16(&arguments[0], &argv, None, Size::new(40, 6)).unwrap()
        };
        let fixture = Self {
            core: SessionCore::new(pty, Terminal::new(40, 6, 32), "local".into(), limit),
            directory,
        };
        fixture.wait_file("ready");
        fixture
    }

    fn wait_file(&self, name: &str) {
        let start = Instant::now();
        while !self.directory.join(name).exists() {
            assert!(start.elapsed() < DEADLINE, "child did not create {name}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn signal(&self, name: &str) {
        fs::write(self.directory.join(name), b"1").unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::write(self.directory.join("stop"), b"1");
        if self.core.pty.wait_timeout(1000).ok().flatten().is_none() {
            let _ = self.core.pty.hangup();
            let status = self.core.pty.wait_timeout(1000).ok().flatten();
            assert!(status.is_some(), "test child was not reaped");
        }
        fs::remove_dir_all(&self.directory).unwrap();
    }
}

fn child_wait(directory: &Path, name: &str) -> bool {
    let start = Instant::now();
    while !directory.join(name).exists() {
        if directory.join("stop").exists() {
            return false;
        }
        assert!(start.elapsed() < DEADLINE, "parent did not signal {name}");
        std::thread::sleep(Duration::from_millis(2));
    }
    true
}

fn child_write(directory: &Path, marker: &str, bytes: &[u8]) {
    std::io::stdout().write_all(bytes).unwrap();
    std::io::stdout().flush().unwrap();
    fs::write(directory.join(marker), b"1").unwrap();
}

fn child(mode: &str, directory: &Path) {
    match mode {
        "tail" => child_write(directory, "ready", &vec![b'x'; 2048]),
        "utf8" => {
            child_write(directory, "ready", b"\xff\xf0\x9f");
            if child_wait(directory, "continue") {
                child_write(directory, "finished", b"\x9a\x80Z\xf0\x9f");
            }
        }
        "history" => {
            child_write(
                directory,
                "ready",
                b"a\r\nb\r\nc\r\nd\r\ne\r\nf\r\ng\r\nh\r\n",
            );
            if !child_wait(directory, "clear") {
                return;
            }
            child_write(directory, "cleared", b"\x1b[3J");
            if child_wait(directory, "grow") {
                child_write(directory, "finished", b"i\r\nj\r\nk\r\nl\r\nm\r\nn\r\n");
            }
        }
        "full-history" => {
            let lines = |start, end| -> String {
                (start..end).map(|line| format!("L{line:03}\r\n")).collect()
            };
            child_write(directory, "ready", lines(0, 60).as_bytes());
            if !child_wait(directory, "grow") {
                return;
            }
            child_write(directory, "grown", lines(60, 65).as_bytes());
            if child_wait(directory, "evict") {
                child_write(directory, "finished", lines(65, 105).as_bytes());
            }
        }
        "exit" => {
            fs::write(directory.join("ready"), b"1").unwrap();
        }
        "idle" | "resize" => {
            fs::write(directory.join("ready"), b"1").unwrap();
            if child_wait(directory, "measure") {
                #[cfg(unix)]
                if mode == "resize" {
                    let size = kernel_size(0);
                    fs::write(
                        directory.join("size"),
                        format!("{} {}", size.cols, size.rows),
                    )
                    .unwrap();
                }
            }
        }
        _ => panic!("unknown test child mode"),
    }
}

fn queue_is_bounded_and_atomic() {
    let mut queue = InputQueue::new(4);
    assert!(!queue.pending());
    assert!(queue.push(b"abcd"));
    assert!(!queue.push(b"e"));
    assert_eq!(queue.pending_len(), 4);
    assert!(queue.push(b""));
    queue.clear();
    assert_eq!(queue.pending_len(), 0);
    assert!(queue.push(b"xyz"));
    assert!(!queue.push(b"12"));
    assert_eq!(queue.pending_len(), 3);
    assert!(queue.push(b"0"));
    let mut disabled = InputQueue::new(0);
    assert!(disabled.push(b""));
    assert!(!disabled.push(b"x"));
}

fn ownership_cancels_only_on_change() {
    let mut fixture = Fixture::new("idle", 4096);
    let core = &mut fixture.core;
    assert!(session_control::valid_actor("agent:core"));
    assert_eq!(core.owner(), Some("local"));
    assert!(core.owns("local"));
    assert!(core.input.push(b"preserved"));
    for takeover in [false, true] {
        assert_eq!(core.acquire("local", takeover), Ok(false));
        assert_eq!(core.input.pending_len(), 9);
    }
    assert!(core.acquire("agent:core", false).is_err());
    assert!(core.release("agent:core").is_err());
    assert_eq!(core.input.pending_len(), 9);
    assert_eq!(core.acquire("agent:core", true), Ok(true));
    assert!(!core.input.pending());
    assert!(!core.owns("local"));
    assert!(core.input.push(b"cancelled"));
    core.release("agent:core").unwrap();
    assert_eq!(core.owner(), None);
    assert!(!core.input.pending());
    assert_eq!(core.acquire("local", false), Ok(true));
}

fn rejected_replies_are_cleared() {
    let mut fixture = Fixture::new("idle", 4);
    let core = &mut fixture.core;
    core.terminal.feed(b"\x1b[5n");
    assert_eq!(core.terminal.responses(), b"\x1b[0n");
    assert!(core.queue_responses());
    assert_eq!(core.input.pending_len(), 4);
    core.terminal.feed(b"\x1b[6n");
    assert!(!core.queue_responses());
    assert!(core.terminal.responses().is_empty());
    assert_eq!(core.input.pending_len(), 4);
    core.input.clear();
    assert!(core.queue_responses());
    assert!(!core.input.pending());
}

fn resize_commits_only_after_pty_accepts() {
    let mut fixture = Fixture::new("resize", 4096);
    assert_eq!(
        fixture.core.read_once(&mut [], false).unwrap(),
        ReadStep::Pending
    );
    assert!(!fixture.core.eof());
    let empty = fixture.core.read_output(0, None).unwrap();
    assert!(empty.bytes.is_empty());
    assert_eq!((empty.first_cursor, empty.next_cursor), (0, 0));
    assert!(!empty.truncated && !empty.has_more);
    fixture.core.flush_input().unwrap();
    fixture.core.terminal.set_cell_size(7, 13);
    fixture.core.terminal.clear_dirty();
    let error = fixture
        .core
        .resize_with_cell_size(Size::new(0, 24), (11, 19))
        .unwrap_err();
    assert_eq!(error.operation, zpty::Operation::Resize);
    assert_eq!(
        (fixture.core.terminal.cols(), fixture.core.terminal.rows()),
        (40, 6)
    );
    assert!(!fixture.core.terminal.dirty());
    fixture.core.terminal.feed(b"\x1b[16t");
    assert_eq!(fixture.core.terminal.responses(), b"\x1b[6;13;7t");
    fixture.core.terminal.clear_responses();
    fixture.core.resize(Size::new(83, 29)).unwrap();
    assert_eq!(
        (fixture.core.terminal.cols(), fixture.core.terminal.rows()),
        (83, 29)
    );
    fixture
        .core
        .resize_with_cell_size(Size::new(89, 27), (11, 19))
        .unwrap();
    fixture.core.terminal.feed(b"\x1b[16t");
    assert_eq!(fixture.core.terminal.responses(), b"\x1b[6;19;11t");
    fixture.core.terminal.clear_responses();
    fixture.signal("measure");
    #[cfg(unix)]
    {
        fixture.wait_file("size");
        assert_eq!(
            fs::read_to_string(fixture.directory.join("size")).unwrap(),
            "89 27"
        );
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn ioctl(fd: i32, request: core::ffi::c_ulong, ...) -> i32;
    fn dup2(source: i32, destination: i32) -> i32;
}

#[cfg(unix)]
fn kernel_size(fd: i32) -> Size {
    #[cfg(target_os = "linux")]
    const TIOCGWINSZ: core::ffi::c_ulong = 0x5413;
    #[cfg(target_os = "macos")]
    const TIOCGWINSZ: core::ffi::c_ulong = 0x40087468;
    let mut size = Size::new(0, 0);
    assert_eq!(unsafe { ioctl(fd, TIOCGWINSZ, &mut size) }, 0);
    size
}

#[cfg(unix)]
fn drain_phase(core: &mut SessionCore, history: &mut usize) -> Vec<ReadStep> {
    let start = Instant::now();
    let mut steps = Vec::new();
    let mut buffer = [0; 2];
    loop {
        assert!(start.elapsed() < DEADLINE, "read did not yield");
        let step = core.read_once(&mut buffer, false).unwrap();
        match step {
            ReadStep::Data {
                history_scrolled,
                history_after,
                ..
            } => {
                if *history != 0 {
                    *history = history.saturating_add(history_scrolled).min(history_after);
                }
                steps.push(step);
            }
            ReadStep::Pending | ReadStep::Eof => return steps,
        }
    }
}

#[cfg(unix)]
fn drain_to_eof(core: &mut SessionCore) {
    let start = Instant::now();
    let mut buffer = [0; 7];
    while !core.eof() {
        assert!(start.elapsed() < DEADLINE, "output did not reach EOF");
        if core.read_once(&mut buffer, false).unwrap() == ReadStep::Pending {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    assert!(core.pty.wait_timeout(1000).unwrap().unwrap().success());
}

#[cfg(unix)]
fn reaped_child_does_not_discard_buffered_output() {
    let mut fixture = Fixture::new("tail", 4096);
    assert!(
        fixture
            .core
            .pty
            .wait_timeout(1000)
            .unwrap()
            .unwrap()
            .success()
    );
    assert!(!fixture.core.eof());
    drain_to_eof(&mut fixture.core);
    let chunk = fixture.core.read_output(0, None).unwrap();
    assert_eq!(chunk.bytes, vec![b'x'; 2048]);
    assert_eq!(chunk.next_cursor, 2048);
    assert!(!chunk.has_more && !chunk.truncated);
    assert_eq!(
        fixture.core.read_once(&mut [0; 7], false).unwrap(),
        ReadStep::Eof
    );
    assert_eq!(
        fixture.core.read_output(2048, None).unwrap().next_cursor,
        2048
    );
}

#[cfg(unix)]
fn fragmented_utf8_waits_for_completion_and_flushes_at_eof() {
    let mut fixture = Fixture::new("utf8", 4096);
    drain_phase(&mut fixture.core, &mut 0);
    let first = fixture.core.read_output(0, None).unwrap();
    assert_eq!(first.bytes, b"\xff");
    assert_eq!(first.next_cursor, 1);
    assert!(first.has_more);
    let waiting = fixture.core.read_output(1, Some(1)).unwrap();
    assert!(waiting.bytes.is_empty());
    assert_eq!(waiting.next_cursor, 1);
    assert!(!fixture.core.eof());
    fixture.signal("continue");
    drain_to_eof(&mut fixture.core);
    let rest = fixture.core.read_output(1, None).unwrap();
    assert_eq!(String::from_utf8_lossy(&rest.bytes), "🚀Z�");
    assert_eq!(rest.next_cursor, 8);
    assert!(!rest.has_more);
}

#[cfg(unix)]
fn history_clear_resets_the_anchor_before_later_growth() {
    let mut fixture = Fixture::new("history", 4096);
    drain_phase(&mut fixture.core, &mut 0);
    assert!(fixture.core.terminal.history_len() > 0);
    let mut history = fixture.core.terminal.history_len();
    fixture.signal("clear");
    fixture.wait_file("cleared");
    let steps = drain_phase(&mut fixture.core, &mut history);
    assert!(steps.iter().any(|step| matches!(
        step,
        ReadStep::Data {
            history_before,
            history_after: 0,
            ..
        } if *history_before > 0
    )));
    assert_eq!(history, 0);
    fixture.signal("grow");
    fixture.wait_file("finished");
    drain_phase(&mut fixture.core, &mut history);
    assert!(fixture.core.terminal.history_len() > 0);
    assert_eq!(history, 0);
    drain_to_eof(&mut fixture.core);
}

#[cfg(unix)]
fn full_history_keeps_the_visible_rows_until_they_are_evicted() {
    let mut fixture = Fixture::new("full-history", 4096);
    drain_phase(&mut fixture.core, &mut 0);
    assert_eq!(fixture.core.terminal.history_len(), 32);
    let mut history = 10;
    let visible = |terminal: &Terminal, offset| -> String {
        (0..terminal.cols())
            .map(|col| terminal.view_cell(col, 0, offset).ch)
            .collect()
    };
    let original = visible(&fixture.core.terminal, history);
    assert!(original.starts_with("L045"), "{original:?}");
    fixture.signal("grow");
    fixture.wait_file("grown");
    drain_phase(&mut fixture.core, &mut history);
    assert_eq!(fixture.core.terminal.history_len(), 32);
    assert_eq!(history, 15);
    assert_eq!(visible(&fixture.core.terminal, history), original);
    fixture.signal("evict");
    fixture.wait_file("finished");
    drain_phase(&mut fixture.core, &mut history);
    assert_eq!(history, 32);
    let oldest = visible(&fixture.core.terminal, history);
    assert!(oldest.starts_with("L068"), "{oldest:?}");
    drain_to_eof(&mut fixture.core);
}

#[cfg(unix)]
fn kernel_errors_preserve_pending_input_and_terminal_size() {
    use std::os::fd::AsRawFd;
    let descriptors_before = fs::read_dir("/dev/fd").unwrap().count();
    {
        let mut fixture = Fixture::new("exit", 4096);
        assert!(
            fixture
                .core
                .pty
                .wait_timeout(1000)
                .unwrap()
                .unwrap()
                .success()
        );
        assert!(!fixture.core.eof());
        let fd = fixture.core.pty.fd();
        let replace_owned_fd = |write: bool| {
            let file = fs::OpenOptions::new()
                .read(!write)
                .write(write)
                .open("/dev/null")
                .unwrap();
            // Only this test's already-reaped PTY is replaced. dup2 closes the
            // old descriptor atomically; Pty remains the sole owner of the
            // destination and will close it exactly once. No model is mocked.
            assert_eq!(unsafe { dup2(file.as_raw_fd(), fd) }, fd);
        };
        replace_owned_fd(true);
        let error = fixture.core.read_once(&mut [0; 16], false).unwrap_err();
        assert_eq!((error.operation, error.errno), (zpty::Operation::Read, 9));
        assert!(fixture.core.eof());
        assert_eq!(
            fixture.core.read_once(&mut [0; 16], false).unwrap(),
            ReadStep::Eof
        );
        replace_owned_fd(false);
        assert!(fixture.core.input.push(b"retained"));
        let error = fixture.core.flush_input().unwrap_err();
        assert_eq!((error.operation, error.errno), (zpty::Operation::Write, 9));
        assert_eq!(fixture.core.input.pending_len(), 8);
        fixture.core.terminal.clear_dirty();
        let error = fixture.core.resize(Size::new(91, 32)).unwrap_err();
        assert_eq!(
            (error.operation, error.errno),
            (zpty::Operation::Resize, 25)
        );
        assert_eq!(
            (fixture.core.terminal.cols(), fixture.core.terminal.rows()),
            (40, 6)
        );
        assert!(!fixture.core.terminal.dirty());
    }
    assert_eq!(fs::read_dir("/dev/fd").unwrap().count(), descriptors_before);
}

fn main() {
    let arguments: Vec<_> = std::env::args_os().collect();
    if arguments
        .get(1)
        .is_some_and(|argument| argument == "--session-core-child")
    {
        child(arguments[2].to_str().unwrap(), Path::new(&arguments[3]));
        return;
    }
    let mut checks = 0;
    let mut run = |name: &str, test: fn()| {
        print!("{name} ... ");
        std::io::stdout().flush().unwrap();
        test();
        checks += 1;
        println!("ok");
    };
    run("bounded atomic input", queue_is_bounded_and_atomic);
    run(
        "control and queue cancellation",
        ownership_cancels_only_on_change,
    );
    run("bounded terminal responses", rejected_replies_are_cleared);
    run(
        "transactional PTY resize",
        resize_commits_only_after_pty_accepts,
    );
    #[cfg(unix)]
    {
        run(
            "reaped child output drain",
            reaped_child_does_not_discard_buffered_output,
        );
        run(
            "fragmented UTF-8 and EOF",
            fragmented_utf8_waits_for_completion_and_flushes_at_eof,
        );
        run(
            "per-feed history anchor",
            history_clear_resets_the_anchor_before_later_growth,
        );
        run(
            "full-history viewport retention",
            full_history_keeps_the_visible_rows_until_they_are_evicted,
        );
        run(
            "native errors and descriptor cleanup",
            kernel_errors_preserve_pending_input_and_terminal_size,
        );
    }
    #[cfg(windows)]
    println!("Unix raw-byte output and descriptor checks: skipped (ConPTY is a VT encoder).");
    println!("SessionCore: {checks} checks passed; every test child joined.");
}
