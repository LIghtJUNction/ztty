//! Terminal views of persistent Linux PTYs, carried by the system SSH client.
use crate::{
    broker,
    json::{self, Value},
    os,
};
use alloc::{ffi::CString, format, string::String, vec::Vec};
use core::{
    ffi::CStr,
    sync::atomic::{AtomicBool, AtomicI32, Ordering},
};
use ztty::{
    mux_input::DetachInput,
    ssh_command::{SshAction, SshOptions},
    terminal_frame::Renderer,
};

const HELP: &str = "Usage: ztty ssh [OPTIONS] HOST [SESSION] [-- COMMAND [ARG...]]\n\
       ztty mux attach [--observe] [--takeover] NAME [-- COMMAND [ARG...]]\n\
       ztty mux list | close NAME\n\n\
SSH opens a native window; --terminal uses the current terminal.\n\
  --list / --close     List sessions or explicitly terminate SESSION\n\
  --observe           Attach without taking input control\n\
  --takeover          Explicitly replace the current input owner\n\
  -p PORT / -i FILE    SSH port / identity file\n\
  -F FILE             SSH configuration file\n\
  --remote-bin PATH   Remote ztty executable (default: PATH or ~/.local/bin)\n\
  --tab TARGET        Add a remote session to a local window\n\
  --split-right TARGET / --split-down TARGET  Add a remote pane\n\n\
HOST accepts an SSH alias, user@host, or ssh://user@host:port.\n\
SESSION defaults to main. Attaching creates it only when absent.\n\
Ctrl+] detaches; closing the view or SSH connection preserves the remote PTY.\n\
Remote sessions require ztty 0.6.0+ on Linux.\n";

fn text<'a>(args: &[&'a CStr]) -> Result<Vec<&'a str>, String> {
    args.iter()
        .map(|a| {
            a.to_str()
                .map_err(|_| String::from("arguments must be UTF-8"))
        })
        .collect()
}

pub fn ssh(args: &[&CStr]) -> Result<i32, String> {
    if args == [c"--help"] || args == [c"-h"] {
        os::print(1, HELP.as_bytes());
        return Ok(0);
    }
    let args = text(args)?;
    let options = SshOptions::parse(&args)?;
    let control = control_path()?;
    let command = options.ssh_argv(Some(&control))?;
    if options.action != SshAction::Attach || options.terminal {
        let mut pointers: Vec<_> = command.iter().map(|a| a.as_ptr()).collect();
        pointers.push(core::ptr::null());
        unsafe {
            libc::execvp(command[0].as_ptr(), pointers.as_ptr());
        }
        return Err(format!("cannot execute ssh (OS error {})", os::errno()));
    }
    let profile = make_profile(&options, &control);
    let mut launch = alloc::vec![CString::from(c"ztty")];
    if let Some((target, layout)) = options.target {
        launch.push(CString::from(match layout.as_str() {
            "right" => c"--split-right",
            "down" => c"--split-down",
            _ => c"--tab",
        }));
        launch.push(CString::new(target).map_err(|_| "invalid local target")?);
    }
    launch.push(c"-e".into());
    launch.extend(command);
    let refs: Vec<_> = launch.iter().map(|s| s.as_c_str()).collect();
    let mut config = crate::config(&refs)?.ok_or("missing SSH window configuration")?;
    config.ssh_profile = Some(profile);
    crate::platform::run(config)
}

fn make_profile(options: &SshOptions, control: &str) -> Value {
    let mut args = Vec::new();
    if let Some(port) = options.port {
        args.extend([Value::from("-p"), Value::from(format!("{port}"))]);
    }
    for (flag, value) in [
        ("-i", options.identity.as_deref()),
        ("-F", options.ssh_config.as_deref()),
        ("--remote-bin", Some(options.remote_bin.as_str())),
    ] {
        if let Some(value) = value {
            args.extend([Value::from(flag), Value::from(value)]);
        }
    }
    args.push(options.destination.as_str().into());
    json::object([
        ("args", Value::Array(args)),
        ("control_path", control.into()),
        ("destination", options.destination.as_str().into()),
        ("session", options.session.as_str().into()),
    ])
}

fn profile_options(profile: &Value) -> Result<(SshOptions, &str), String> {
    let values = profile
        .get("args")
        .and_then(Value::as_array)
        .filter(|v| v.len() <= 16)
        .ok_or("invalid SSH workspace profile")?;
    let args: Vec<&str> = values
        .iter()
        .map(|v| v.as_str().ok_or("invalid SSH profile argument"))
        .collect::<Result<_, _>>()?;
    let options = SshOptions::parse(&args)?;
    if options.action != SshAction::Attach
        || options.terminal
        || options.observe
        || options.takeover
        || options.target.is_some()
        || !options.command.is_empty()
    {
        return Err("invalid SSH workspace connection options".into());
    }
    let control = profile
        .get("control_path")
        .and_then(Value::as_str)
        .filter(|v| v.starts_with('/') && v.len() <= 100 && !v.chars().any(char::is_control))
        .ok_or("invalid SSH workspace control path")?;
    if profile.get("destination").and_then(Value::as_str) != Some(options.destination.as_str())
        || !profile
            .get("session")
            .and_then(Value::as_str)
            .is_some_and(valid_name)
    {
        return Err("invalid SSH workspace identity".into());
    }
    Ok((options, control))
}

pub(crate) fn validate_profile(profile: &Value) -> Result<(), String> {
    let (options, control) = profile_options(profile)?;
    options.ssh_argv(Some(control)).map(|_| ())
}

/// Native tab/split actions inherit the SSH connection, but start their own
/// named remote shell. A previous pane's one-shot command is never replayed.
pub(crate) fn related_command(
    profile: &Value,
    name: &str,
) -> Result<(Value, Vec<CString>), String> {
    let (mut options, control) = profile_options(profile)?;
    options.session = name.into();
    let command = options.ssh_argv(Some(control))?;
    Ok((make_profile(&options, control), command))
}

/// OpenSSH owns authentication, host-key verification and the reusable socket.
/// A private directory prevents another user from substituting that socket.
fn control_path() -> Result<String, String> {
    let uid = unsafe { os::getuid() };
    let directory = match os::env(c"XDG_RUNTIME_DIR").and_then(|s| core::str::from_utf8(s).ok()) {
        Some(base) if base.starts_with('/') && base.len() < 50 => format!("{base}/ztty-ssh"),
        _ => format!("/tmp/ztty-ssh-{uid}"),
    };
    let name = CString::new(directory.as_str()).map_err(|_| "invalid SSH runtime directory")?;
    if unsafe { libc::mkdir(name.as_ptr(), 0o700) } < 0 && os::errno() != libc::EEXIST {
        return Err("cannot create private SSH connection directory".into());
    }
    let mut stat: libc::stat = unsafe { core::mem::zeroed() };
    if unsafe { libc::lstat(name.as_ptr(), &mut stat) } < 0
        || stat.st_uid != uid
        || stat.st_mode & libc::S_IFMT != libc::S_IFDIR
        || stat.st_mode & 0o077 != 0
    {
        return Err("SSH connection directory must be owned by you with mode 0700".into());
    }
    Ok(format!("{directory}/%C"))
}

fn select_broker() -> Result<(), String> {
    if os::env(c"ZTTY_SOCKET").is_none() {
        let name = CString::new(format!("@ztty-{}-mux-v1", unsafe { os::getuid() }))
            .map_err(|_| "invalid mux socket")?;
        if unsafe { libc::setenv(c"ZTTY_SOCKET".as_ptr(), name.as_ptr(), 0) } != 0 {
            return Err("cannot select persistent terminal broker".into());
        }
    }
    Ok(())
}

fn process_start(pid: u32) -> Option<u64> {
    let path = CString::new(format!("/proc/{pid}/stat")).ok()?;
    let bytes = os::read_config(&path).ok()?;
    let stat = core::str::from_utf8(&bytes).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_ascii_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

fn abandoned_owner(owner: &str) -> bool {
    let Some((pid, start)) = owner.strip_prefix("mux:").and_then(|s| s.split_once(':')) else {
        return false;
    };
    let (Ok(pid), Ok(start)) = (pid.parse::<u32>(), start.parse::<u64>()) else {
        return false;
    };
    process_start(pid) != Some(start)
}

struct Lease {
    session: String,
    expected: Value,
    actor: String,
    owns: bool,
}

impl Lease {
    fn request(&self, op: &str) -> Value {
        json::object([
            ("op", op.into()),
            ("session", self.session.as_str().into()),
            ("expected_session", self.expected.clone()),
            ("_actor", self.actor.as_str().into()),
        ])
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if self.owns {
            let mut request = self.request("control");
            request.set("action", "release".into());
            let _ = broker::request(&request);
        }
    }
}

pub fn command(args: &[&CStr]) -> Result<i32, String> {
    if args.is_empty() || args == [c"--help"] || args == [c"-h"] {
        os::print(1, HELP.as_bytes());
        return Ok(0);
    }
    let args = text(args)?;
    select_broker()?;
    if args == ["list"] {
        let value = broker::request(&json::object([("op", "list".into())]))?;
        os::print(1, json::stringify(&value).as_bytes());
        os::print(1, b"\n");
        return Ok(0);
    }
    let mut observe = false;
    let mut takeover = false;
    let mut index = 1;
    while let Some(option) = args.get(index) {
        match *option {
            "--observe" => observe = true,
            "--takeover" => takeover = true,
            _ => break,
        }
        index += 1;
    }
    if observe && takeover {
        return Err("--observe and --takeover cannot be combined".into());
    }
    let name = *args.get(index).ok_or("session name is required")?;
    if !valid_name(name) {
        return Err(
            "session names use 1..64 ASCII letters, digits, dot, underscore or hyphen".into(),
        );
    }
    let command = &args[index + 1..];
    if !command.is_empty() && (command[0] != "--" || command.len() == 1) {
        return Err("place -- before the command and its arguments".into());
    }
    if !matches!(args[0], "attach" | "close")
        || args[0] == "close" && (index != 1 || !command.is_empty())
        || observe && !command.is_empty()
    {
        return Err("usage: ztty mux attach [--observe|--takeover] NAME [-- COMMAND...] | list | close NAME".into());
    }
    // Validate the terminal before creating a session. Listing/closing are also
    // useful from noninteractive SSH commands and do not need a PTY.
    if args[0] == "attach" && (unsafe { libc::isatty(0) } != 1 || unsafe { libc::isatty(1) } != 1) {
        return Err("mux attach needs a terminal; use ssh -tt or ztty ssh".into());
    }
    let pid = unsafe { os::getpid() } as u32;
    let actor = format!(
        "mux:{pid}:{}",
        process_start(pid).ok_or("cannot identify mux process")?
    );
    let size = terminal_size();
    let info_request = json::object([("op", "info".into()), ("session", name.into())]);
    let info = match broker::request(&info_request) {
        Ok(info) => info,
        Err(error)
            if args[0] == "attach" && !observe && error.starts_with("session not found:") =>
        {
            let mut create = json::object([
                ("op", "create".into()),
                ("name", name.into()),
                ("headless", true.into()),
                ("origin", "mux".into()),
                ("cols", (size.0 as u64).into()),
                ("rows", (size.1 as u64).into()),
                ("_actor", actor.as_str().into()),
            ]);
            if !command.is_empty() {
                create.set(
                    "command",
                    Value::Array(command[1..].iter().map(|s| Value::from(*s)).collect()),
                );
            }
            match broker::request(&create) {
                Ok(value) => value,
                // Concurrent clients can race to create the same named session.
                // The winner alone starts the command; the other attaches it.
                Err(error) if error.starts_with("session already exists:") => {
                    broker::request(&info_request)?
                }
                Err(error) => return Err(error),
            }
        }
        Err(error) => return Err(error),
    };
    let expected = json::object([
        (
            "uri",
            info.get("uri").cloned().ok_or("missing session URI")?,
        ),
        ("id", info.get("id").cloned().ok_or("missing session id")?),
        (
            "created_at",
            info.get("created_at")
                .cloned()
                .ok_or("missing session creation time")?,
        ),
        (
            "broker_pid",
            info.get("broker_pid")
                .cloned()
                .ok_or("missing broker identity")?,
        ),
    ]);
    let mut lease = Lease {
        session: name.into(),
        expected,
        actor,
        owns: false,
    };
    if !observe {
        let mut acquire = lease.request("control");
        acquire.set("action", "acquire".into());
        acquire.set(
            "takeover",
            (takeover
                || args[0] == "close"
                || info
                    .get("input_owner")
                    .and_then(Value::as_str)
                    .is_some_and(abandoned_owner))
            .into(),
        );
        broker::request(&acquire)
            .map_err(|error| format!("{error}; use --observe or --takeover"))?;
        lease.owns = true;
    }
    if args[0] == "close" {
        let result = broker::request(&lease.request("close"))?;
        lease.owns = false;
        os::print(1, json::stringify(&result).as_bytes());
        os::print(1, b"\n");
        return Ok(0);
    }
    let terminal = RawTerminal::new()?;
    let result = attach_loop(&lease, observe, size);
    drop(terminal);
    drop(lease);
    result
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

static STOP: AtomicI32 = AtomicI32::new(0);
static RESIZE: AtomicBool = AtomicBool::new(true);

extern "C" fn signal(number: i32) {
    if number == libc::SIGWINCH {
        RESIZE.store(true, Ordering::Relaxed);
    } else {
        STOP.store(number, Ordering::Relaxed);
    }
}

struct RawTerminal {
    saved: libc::termios,
    flags: [i32; 2],
    signals: Vec<(i32, libc::sigaction)>,
}

impl RawTerminal {
    fn new() -> Result<Self, String> {
        let mut saved = unsafe { core::mem::zeroed() };
        if unsafe { libc::tcgetattr(0, &mut saved) } != 0 {
            return Err("cannot read terminal attributes".into());
        }
        let mut value = Self {
            saved,
            flags: [unsafe { libc::fcntl(0, libc::F_GETFL) }, unsafe {
                libc::fcntl(1, libc::F_GETFL)
            }],
            signals: Vec::new(),
        };
        if value.flags.iter().any(|f| *f < 0) {
            return Err("cannot inspect terminal descriptors".into());
        }
        STOP.store(0, Ordering::Relaxed);
        RESIZE.store(true, Ordering::Relaxed);
        for number in [
            libc::SIGHUP,
            libc::SIGINT,
            libc::SIGQUIT,
            libc::SIGTERM,
            libc::SIGWINCH,
            libc::SIGPIPE,
        ] {
            let mut action: libc::sigaction = unsafe { core::mem::zeroed() };
            action.sa_sigaction = if number == libc::SIGPIPE {
                libc::SIG_IGN
            } else {
                signal as *const () as usize
            };
            let mut old = unsafe { core::mem::zeroed() };
            if unsafe { libc::sigaction(number, &action, &mut old) } != 0 {
                return Err("cannot install terminal signal handlers".into());
            }
            value.signals.push((number, old));
        }
        let mut raw = saved;
        unsafe {
            libc::cfmakeraw(&mut raw);
        }
        if unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw) } != 0 {
            return Err("cannot enter raw terminal mode".into());
        }
        for fd in 0..=1 {
            if unsafe {
                libc::fcntl(
                    fd,
                    libc::F_SETFL,
                    value.flags[fd as usize] | libc::O_NONBLOCK,
                )
            } < 0
            {
                return Err("cannot configure nonblocking terminal I/O".into());
            }
        }
        Ok(value)
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        reset_output(b"\x1b[?2026l\x1b[?1049l\x1b[?1l\x1b>\x1b[?2004l\x1b[?9l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1004l\x1b[=0u\x1b[?7h\x1b[?25h\x1b[0 q\x1b[0m\r\n");
        unsafe {
            libc::tcsetattr(0, libc::TCSANOW, &self.saved);
            for fd in 0..=1 {
                if self.flags[fd as usize] >= 0 {
                    libc::fcntl(fd, libc::F_SETFL, self.flags[fd as usize]);
                }
            }
            for (number, action) in &self.signals {
                libc::sigaction(*number, action, core::ptr::null_mut());
            }
        }
    }
}

fn reset_output(mut bytes: &[u8]) {
    let deadline = os::milliseconds().saturating_add(250);
    while !bytes.is_empty() && os::milliseconds() < deadline {
        let n = unsafe { os::write(1, bytes.as_ptr().cast(), bytes.len()) };
        if n > 0 {
            bytes = &bytes[n as usize..];
            continue;
        }
        if n < 0 && os::errno() == os::EINTR {
            continue;
        }
        if n >= 0 || os::errno() != libc::EAGAIN {
            break;
        }
        let mut fd = os::PollFd {
            fd: 1,
            events: os::POLLOUT,
            revents: 0,
        };
        unsafe {
            os::poll(
                &mut fd,
                1,
                deadline.saturating_sub(os::milliseconds()) as i32,
            );
        }
    }
}

fn terminal_size() -> (u16, u16) {
    let mut size: libc::winsize = unsafe { core::mem::zeroed() };
    if unsafe { libc::ioctl(0, libc::TIOCGWINSZ, &mut size) } != 0
        || size.ws_col == 0
        || size.ws_row == 0
    {
        (120, 32)
    } else {
        (size.ws_col.clamp(2, 512), size.ws_row.clamp(2, 256))
    }
}

fn resize_request(lease: &Lease, size: (u16, u16)) -> Value {
    let mut value = lease.request("resize");
    value.set("cols", (size.0 as u64).into());
    value.set("rows", (size.1 as u64).into());
    value
}

fn write_request(lease: &Lease, data: &[u8]) -> Value {
    let mut value = lease.request("write");
    value.set("_data_hex", Value::from(broker::hex(data)));
    value.set("_drain_ms", broker::MAX_INPUT_DRAIN_MS.into());
    value
}

fn attach_loop(lease: &Lease, observe: bool, mut size: (u16, u16)) -> Result<i32, String> {
    let mut control = if observe {
        None
    } else {
        Some(broker::PendingRequest::new(&resize_request(lease, size))?)
    };
    let mut snapshot: Option<broker::PendingRequest> = None;
    let mut watch: Option<broker::PendingRequest> = None;
    let mut history = None;
    let mut renderer = Renderer::new();
    let mut output = Vec::new();
    let mut sent = 0;
    let mut need_frame = true;
    let mut next_frame = 0;
    let mut revision = 0;
    let mut epoch = 0;
    let mut exited = None;
    let mut input = [0u8; 65536 - 128];
    let mut detacher = DetachInput::new();
    let mut escape_deadline = None;
    let mut detach_after_write = false;
    let mut draining_input = false;
    let mut fds = Vec::with_capacity(5);
    loop {
        let stopped = STOP.load(Ordering::Relaxed);
        if stopped != 0 {
            return Ok(128 + stopped);
        }
        if let Some(pending) = &mut control
            && let Some(result) = pending.advance()
        {
            let value = result?;
            if draining_input && value.get("drained").and_then(Value::as_bool) != Some(true) {
                return Err("broker did not confirm pending input was accepted by the PTY".into());
            }
            draining_input = false;
            control = None;
        }
        if detach_after_write && control.is_none() {
            return Ok(0);
        }
        if control.is_none() && escape_deadline.is_some_and(|d| os::milliseconds() >= d) {
            let data = detacher.flush();
            escape_deadline = None;
            if !observe && !data.is_empty() {
                control = Some(broker::PendingRequest::new(&write_request(lease, &data))?);
                draining_input = true;
            }
        }
        if control.is_none() && RESIZE.swap(false, Ordering::Relaxed) {
            let next = terminal_size();
            if next != size {
                size = next;
                history = None;
                watch = None;
                snapshot = None;
                need_frame = true;
                if !observe {
                    control = Some(broker::PendingRequest::new(&resize_request(lease, size))?);
                }
            }
        }
        if let Some(pending) = &mut watch
            && let Some(result) = pending.advance()
        {
            let value = result?;
            need_frame = value.get("revision").and_then(Value::as_u64) != Some(revision)
                || value.get("control_epoch").and_then(Value::as_u64) != Some(epoch)
                || value.get("output_eof").and_then(Value::as_bool) == Some(true);
            watch = None;
        }
        if let Some(pending) = &mut snapshot
            && let Some(result) = pending.advance()
        {
            let frame: Value = result?;
            let info = frame
                .get("session")
                .ok_or("missing frame session metadata")?;
            if !observe
                && info.get("input_owner").and_then(Value::as_str) != Some(lease.actor.as_str())
            {
                return Err(
                    "input control moved to another client; reconnect with --observe or --takeover"
                        .into(),
                );
            }
            if info.get("output_eof").and_then(Value::as_bool) == Some(true) {
                exited = info
                    .get("exit_code")
                    .and_then(Value::as_i64)
                    .map(|code| code as i32);
            }
            output = renderer.render(&frame)?;
            sent = 0;
            revision = frame
                .get("revision")
                .and_then(Value::as_u64)
                .ok_or("missing frame revision")?;
            epoch = frame
                .get("control_epoch")
                .and_then(Value::as_u64)
                .ok_or("missing control epoch")?;
            history = if frame.get("alternate").and_then(Value::as_bool) == Some(true) {
                None
            } else {
                frame.get("history_position").and_then(Value::as_u64)
            };
            snapshot = None;
            next_frame = os::milliseconds().saturating_add(16);
        }
        while sent < output.len() {
            let n = unsafe { os::write(1, output[sent..].as_ptr().cast(), output.len() - sent) };
            if n > 0 {
                sent += n as usize;
            } else if n < 0 && os::errno() == os::EINTR {
                continue;
            } else if n < 0 && os::errno() == libc::EAGAIN {
                break;
            } else {
                return Ok(0);
            }
        }
        if sent == output.len() {
            output.clear();
            sent = 0;
            if let Some(code) = exited {
                return Ok(code);
            }
        }
        let now = os::milliseconds();
        if output.is_empty()
            && snapshot.is_none()
            && control.is_none()
            && need_frame
            && now >= next_frame
        {
            let mut request = lease.request("terminal-frame");
            request.set("view_cols", (size.0 as u64).into());
            request.set("view_rows", (size.1 as u64).into());
            if let Some(history) = history {
                request.set("history_since", Value::from(history));
            }
            snapshot = Some(broker::PendingRequest::new(&request)?);
            need_frame = false;
        } else if !need_frame && snapshot.is_none() && watch.is_none() && exited.is_none() {
            let mut request = lease.request("watch");
            request.set("revision", Value::from(revision));
            request.set("control_epoch", Value::from(epoch));
            request.set("wait_ms", Value::from(30000u64));
            watch = Some(broker::PendingRequest::new(&request)?);
        }
        fds.clear();
        fds.push(os::PollFd {
            fd: 0,
            events: if control.is_none() { os::POLLIN } else { 0 },
            revents: 0,
        });
        fds.push(os::PollFd {
            fd: 1,
            events: if output.is_empty() { 0 } else { os::POLLOUT },
            revents: 0,
        });
        for pending in [&control, &snapshot, &watch].into_iter().flatten() {
            if let Some(fd) = pending.poll_fd() {
                fds.push(fd);
            }
        }
        let mut deadline = [&control, &snapshot, &watch]
            .into_iter()
            .flatten()
            .filter_map(|p| p.next_deadline())
            .min();
        if need_frame && snapshot.is_none() && output.is_empty() && control.is_none() {
            deadline = Some(deadline.map_or(next_frame, |d| d.min(next_frame)));
        }
        if control.is_none()
            && let Some(escape) = escape_deadline
        {
            deadline = Some(deadline.map_or(escape, |d| d.min(escape)));
        }
        let timeout = deadline.map_or(-1, |d| {
            d.saturating_sub(os::milliseconds()).min(i32::MAX as u64) as i32
        });
        let ready = unsafe { os::poll(fds.as_mut_ptr(), fds.len(), timeout) };
        if ready < 0 {
            if os::errno() == os::EINTR {
                continue;
            }
            return Err("terminal poll failed".into());
        }
        if fds[0].revents & (os::POLLHUP | os::POLLERR) != 0 {
            return Ok(0);
        }
        if control.is_none() && fds[0].revents & os::POLLIN != 0 {
            let n = unsafe { os::read(0, input.as_mut_ptr().cast(), input.len()) };
            if n == 0 {
                return Ok(0);
            }
            if n < 0 {
                if matches!(os::errno(), libc::EINTR | libc::EAGAIN) {
                    continue;
                }
                return Ok(0);
            }
            let (data, detach) = detacher.feed(&input[..n as usize]);
            escape_deadline = detacher
                .pending()
                .then(|| os::milliseconds().saturating_add(25));
            if !observe && !data.is_empty() {
                control = Some(broker::PendingRequest::new(&write_request(lease, &data))?);
                draining_input = true;
                detach_after_write = detach;
            } else if detach {
                return Ok(0);
            }
        }
    }
}
