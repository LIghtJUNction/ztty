//! Shared PTYs and desktop windows, driven by one poll loop.
use crate::{
    Appearance, Queue,
    broker::{self, Reply},
    json::{self, Value},
    key, key_modifiers, modifiers, os, paste,
    x11::{self, Connection, Selection, Window},
};
use alloc::{collections::VecDeque, ffi::CString, format, rc::Rc, string::String, vec::Vec};
use core::ffi::{CStr, c_int};
use input::{Key, KeyEvent, MouseEvent};
use zterm::{CursorShape, Terminal, input};

struct Ui {
    window: Window,
    selection: Option<Selection>,
    history: usize,
    force: bool,
    open: bool,
    title: String,
    held: [u64; 256],
    intercepted: [bool; 256],
    local_buttons: [bool; 8],
    swallowed_buttons: [bool; 8],
    sync_since: Option<u64>,
    blink_deadline: Option<u64>,
    cursor_visible: bool,
    last_cursor: Option<((usize, usize), CursorShape)>,
    reset_blink: bool,
}
impl Ui {
    fn new(window: Window) -> Self {
        Self {
            window,
            selection: None,
            history: 0,
            force: true,
            open: true,
            title: String::new(),
            held: [0; 256],
            intercepted: [false; 256],
            local_buttons: [false; 8],
            swallowed_buttons: [false; 8],
            sync_since: None,
            blink_deadline: None,
            cursor_visible: true,
            last_cursor: None,
            reset_blink: false,
        }
    }
    fn event(
        &mut self,
        event: &mut x11::Event,
        terminal: &mut Terminal,
        pty: &zpty::Pty,
        queue: &mut Queue,
        openers: &mut os::Openers,
        text_buffer: &mut Vec<u8>,
        encoded: &mut Vec<u8>,
    ) -> Result<(), &'static str> {
        match unsafe { event.kind } {
            x11::KEY_PRESS | x11::KEY_RELEASE => {
                let mut e = unsafe { event.key };
                let code = (e.keycode as usize).min(255);
                let press = e.kind == x11::KEY_PRESS;
                self.reset_blink |= press;
                let symbol = if !press && self.held[code] != 0 {
                    self.held[code]
                } else {
                    self.window.key_symbol(&mut e, false)
                };
                let repeated = self.held[code] != 0;
                self.held[code] = if press { symbol } else { 0 };
                let mods = key_modifiers(e.state, symbol, press, &self.held);
                if !press && self.intercepted[code] {
                    self.intercepted[code] = false;
                    return Ok(());
                }
                if press && mods.0 & 5 == 5 && (symbol == b'c' as u64 || symbol == b'C' as u64) {
                    if let Some(s) = self.selection {
                        self.window.copy(s.text(&terminal, self.history), false);
                    }
                    self.intercepted[code] = true;
                    return Ok(());
                }
                if press && mods.0 & 5 == 5 && (symbol == b'v' as u64 || symbol == b'V' as u64) {
                    self.window.paste(false);
                    self.intercepted[code] = true;
                    return Ok(());
                }
                if press && mods.0 & 1 != 0 && (symbol == 0xff55 || symbol == 0xff56) {
                    let step = terminal.rows().saturating_sub(1).max(1);
                    self.history = if symbol == 0xff55 {
                        self.history
                            .saturating_add(step)
                            .min(terminal.history_len())
                    } else {
                        self.history.saturating_sub(step)
                    };
                    self.selection = None;
                    self.force = true;
                    self.intercepted[code] = true;
                    return Ok(());
                }
                let lookup = if press {
                    self.window.lookup(&mut e, text_buffer)
                } else {
                    x11::Lookup {
                        len: 0,
                        symbol,
                        text_only: false,
                        ignored: false,
                    }
                };
                if lookup.ignored {
                    self.intercepted[code] = true;
                    return Ok(());
                }
                let text = core::str::from_utf8(&text_buffer[..lookup.len]).unwrap_or("");
                let input_key = if symbol == 0 {
                    key(lookup.symbol)
                } else {
                    key(symbol)
                };
                encoded.clear();
                if lookup.text_only {
                    input::encode_text_into(text, &terminal.modes().input(), encoded);
                    self.intercepted[code] = true;
                } else {
                    let shifted = if mods.0 & 1 != 0 {
                        if let Key::Character(ch) = key(self.window.key_symbol(&mut e, true)) {
                            Some(ch)
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    input::encode_key_with_alternates_into(
                        input_key,
                        mods,
                        if !press {
                            KeyEvent::Release
                        } else if repeated {
                            KeyEvent::Repeat
                        } else {
                            KeyEvent::Press
                        },
                        text,
                        input::KeyAlternates {
                            shifted,
                            base_layout: None,
                        },
                        &terminal.modes().input(),
                        encoded,
                    );
                }
                if !encoded.is_empty() {
                    if self.history != 0 {
                        self.history = 0;
                        self.force = true;
                    }
                    if self.selection.is_some() {
                        self.selection = None;
                        self.force = true;
                    }
                    queue.push(encoded);
                }
            }
            x11::BUTTON_PRESS | x11::BUTTON_RELEASE => {
                let e = unsafe { event.button };
                let (col, row) =
                    self.window
                        .coordinates(e.x, e.y, terminal.cols(), terminal.rows());
                let mods = modifiers(e.state);
                let pressed = e.kind == x11::BUTTON_PRESS;
                let button_index = (e.button as usize).min(7);
                if !pressed && self.swallowed_buttons[button_index] {
                    self.swallowed_buttons[button_index] = false;
                    return Ok(());
                }
                let mut local = mods.0 & 1 != 0 || terminal.modes().mouse_tracking == 0;
                if pressed && local {
                    self.local_buttons[button_index] = true;
                }
                if !pressed {
                    local |= self.local_buttons[button_index];
                    self.local_buttons[button_index] = false;
                }
                if pressed && e.button == 1 && mods.0 & 4 != 0 {
                    let id = terminal.view_cell(col, row, self.history).link_id;
                    if let Some(uri) = terminal.hyperlink(id) {
                        openers.open(uri, self.window.fd, pty.fd());
                        self.swallowed_buttons[button_index] = true;
                        self.local_buttons[button_index] = false;
                        return Ok(());
                    }
                }
                if local {
                    if pressed && e.button == 1 {
                        self.selection = Some(Selection {
                            start: (col, row),
                            end: (col, row),
                            dragging: true,
                        });
                        self.force = true;
                    } else if !pressed && e.button == 1 {
                        if let Some(ref mut s) = self.selection {
                            s.end = (col, row);
                            s.dragging = false;
                            self.window.copy(s.text(&terminal, self.history), true);
                            self.force = true;
                        }
                    } else if pressed && e.button == 2 {
                        self.window.paste(true);
                    } else if pressed && (e.button == 4 || e.button == 5) {
                        self.history = if e.button == 4 {
                            self.history.saturating_add(3).min(terminal.history_len())
                        } else {
                            self.history.saturating_sub(3)
                        };
                        self.selection = None;
                        self.force = true;
                    }
                } else {
                    let mouse = if e.button == 4 || e.button == 5 {
                        if !pressed {
                            return Ok(());
                        }
                        MouseEvent::Scroll {
                            up: e.button == 4,
                            col,
                            row,
                        }
                    } else if (1..=3).contains(&e.button) {
                        if pressed {
                            MouseEvent::Press {
                                button: (e.button - 1) as u8,
                                col,
                                row,
                            }
                        } else {
                            MouseEvent::Release {
                                button: (e.button - 1) as u8,
                                col,
                                row,
                            }
                        }
                    } else {
                        return Ok(());
                    };
                    encoded.clear();
                    input::encode_mouse_into(mouse, mods, &terminal.modes().input(), encoded);
                    queue.push(encoded);
                }
            }
            x11::MOTION_NOTIFY => {
                let e = unsafe { event.motion };
                let (col, row) =
                    self.window
                        .coordinates(e.x, e.y, terminal.cols(), terminal.rows());
                if let Some(ref mut s) = self.selection
                    && s.dragging
                {
                    s.end = (col, row);
                    self.force = true;
                    return Ok(());
                }
                let button = if e.state & 256 != 0 {
                    Some(0)
                } else if e.state & 512 != 0 {
                    Some(1)
                } else if e.state & 1024 != 0 {
                    Some(2)
                } else {
                    None
                };
                encoded.clear();
                input::encode_mouse_into(
                    MouseEvent::Motion { button, col, row },
                    modifiers(e.state),
                    &terminal.modes().input(),
                    encoded,
                );
                queue.push(encoded);
            }
            x11::CONFIGURE_NOTIFY => {
                let e = unsafe { event.configure };
                if e.width != self.window.width || e.height != self.window.height {
                    let (cols, rows) = self.window.resize(e.width, e.height);
                    terminal.resize(cols, rows);
                    pty.resize(zpty::Size {
                        rows: rows as u16,
                        cols: cols as u16,
                        pixel_width: e.width.clamp(0, 65535) as u16,
                        pixel_height: e.height.clamp(0, 65535) as u16,
                    })
                    .map_err(|_| "cannot resize the PTY")?;
                    self.history = self.history.min(terminal.history_len());
                    self.selection = None;
                    self.force = true;
                }
            }
            x11::FOCUS_IN | x11::FOCUS_OUT => {
                let focused = unsafe { event.kind } == x11::FOCUS_IN;
                self.window.focus(focused);
                self.reset_blink = true;
                self.force = true;
                if !focused {
                    self.held.fill(0);
                    self.intercepted.fill(false);
                }
                queue.push(&input::encode_focus(focused, &terminal.modes().input()));
            }
            x11::EXPOSE => {
                self.window.present();
                self.force = true;
            }
            x11::CLIENT_MESSAGE => {
                if unsafe { event.client.data.longs[0] as u64 } == self.window.wm_delete {
                    self.open = false;
                }
            }
            x11::DESTROY_NOTIFY => {
                self.window.id = 0;
                self.open = false;
            }
            x11::SELECTION_REQUEST => {
                self.window.selection_request(unsafe { event.request });
            }
            x11::SELECTION_NOTIFY => {
                if let Some(data) = self.window.selection_notify(unsafe { event.selection }) {
                    paste(data, terminal, queue);
                }
            }
            x11::PROPERTY_NOTIFY => {
                if let Some(data) = self.window.property(unsafe { event.property }) {
                    paste(data, terminal, queue);
                }
            }
            x11::SELECTION_CLEAR => {}
            _ => {}
        }
        Ok(())
    }
    fn paint(&mut self, terminal: &mut Terminal, appearance: &Appearance) -> Option<u64> {
        let now = os::milliseconds();
        if terminal.modes().synchronized_output {
            let start = self.sync_since.get_or_insert(now);
            if now.saturating_sub(*start) >= 150 {
                terminal.force_end_sync();
                self.sync_since = None;
            }
        } else {
            self.sync_since = None;
        }
        if terminal.title() != self.title {
            self.title.clear();
            self.title.push_str(terminal.title());
            self.window.title(&self.title);
        }
        let modes = terminal.modes();
        let cursor = (self.history == 0 && modes.cursor_visible).then(|| {
            (
                terminal.cursor(),
                modes.cursor_shape.unwrap_or(appearance.cursor_shape),
            )
        });
        let blinking = self.window.focused
            && cursor.is_some()
            && modes.cursor_blink.unwrap_or(appearance.cursor_blink);
        let synchronized = modes.synchronized_output;
        if !blinking || synchronized {
            self.blink_deadline = None;
            self.cursor_visible = true;
        } else if self.blink_deadline.is_none() || self.reset_blink || cursor != self.last_cursor {
            self.blink_deadline = Some(now + appearance.cursor_blink_interval);
            self.cursor_visible = true;
        } else if self.blink_deadline.is_some_and(|deadline| deadline <= now) {
            self.blink_deadline = Some(now + appearance.cursor_blink_interval);
            self.cursor_visible = !self.cursor_visible;
        }
        self.last_cursor = cursor;
        self.reset_blink = false;
        let cursor = cursor.filter(|_| self.cursor_visible);
        if !synchronized && (self.force || terminal.dirty() || self.window.cursor_changed(cursor)) {
            self.window
                .render(terminal, self.history, self.selection, self.force, cursor);
            self.force = false;
        }

        match (self.sync_since.map(|s| s + 150), self.blink_deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

struct Session {
    uri: String,
    name: String,
    origin: String,
    appearance: Appearance,
    pty: zpty::Pty,
    terminal: Terminal,
    queue: Queue,
    ui: Option<Ui>,
    persistent: bool,
    eof: bool,
    output: VecDeque<u8>,
    output_end: u64,
}

impl Session {
    fn status(&self) -> Option<i32> {
        self.pty
            .status()
            .map(|s| s.code().unwrap_or(128 + s.signal().unwrap_or(0)))
    }
    fn info(&self) -> Value {
        json::object([
            ("uri", Value::from(self.uri.clone())),
            ("name", Value::from(self.name.clone())),
            ("origin", Value::from(self.origin.clone())),
            ("pid", Value::from(self.pty.pid())),
            ("broker_pid", Value::from(unsafe { os::getpid() })),
            ("cols", Value::from(self.terminal.cols())),
            ("rows", Value::from(self.terminal.rows())),
            ("attached", Value::Bool(self.ui.is_some())),
            (
                "running",
                Value::Bool(!self.eof && self.pty.status().is_none()),
            ),
            (
                "exit_code",
                self.status().map(Value::from).unwrap_or(Value::Null),
            ),
            ("title", Value::from(self.terminal.title())),
        ])
    }
    fn pump(&mut self, buffer: &mut [u8]) {
        if !self.eof {
            for _ in 0..4 {
                let n = match self.pty.read(buffer) {
                    Ok(0) => {
                        self.eof = true;
                        break;
                    }
                    Ok(n) => n,
                    Err(e) if e.is_would_block() => break,
                    Err(_) => {
                        self.eof = true;
                        break;
                    }
                };
                let before = self.terminal.history_len();
                self.terminal.feed(&buffer[..n]);
                if let Some(ui) = &mut self.ui
                    && ui.history != 0
                {
                    ui.history = ui
                        .history
                        .saturating_add(self.terminal.history_len().saturating_sub(before))
                        .min(self.terminal.history_len());
                    ui.force = true;
                }
                let cap = if self.persistent {
                    256 * 1024
                } else {
                    64 * 1024
                };
                self.output_end = self.output_end.saturating_add(n as u64);
                if n >= cap {
                    self.output.clear();
                    self.output.extend(&buffer[n - cap..n]);
                } else {
                    let remove = self.output.len().saturating_add(n).saturating_sub(cap);
                    self.output.drain(..remove);
                    self.output.extend(&buffer[..n]);
                }
            }
        }
        if !self.terminal.responses().is_empty() {
            self.queue.push(self.terminal.responses());
            self.terminal.clear_responses();
        }
        self.queue.flush(&self.pty);
        let _ = self.pty.reap();
    }
    fn read(&self, request: &Value) -> Result<Value, String> {
        let cursor = match request.get("cursor") {
            None => 0,
            Some(v) => v
                .as_u64()
                .ok_or("cursor must be a nonnegative byte offset")?,
        };
        let max = match request.get("max_bytes") {
            None => 16384,
            Some(v) => v
                .as_u64()
                .filter(|&v| v > 0 && v <= 65536)
                .ok_or("max_bytes must be 1..65536")? as usize,
        };
        if cursor > self.output_end {
            return Err("cursor is beyond the available output".into());
        }
        let first = self.output_end - self.output.len() as u64;
        let mut start = cursor.max(first);
        if cursor < first {
            while start < self.output_end && self.output[(start - first) as usize] & 0xc0 == 0x80 {
                start += 1;
            }
        }
        let mut bytes: Vec<u8> = self
            .output
            .iter()
            .skip((start - first) as usize)
            .take(max + 3)
            .copied()
            .collect();
        if bytes.len() > max {
            let mut end = max;
            while end < bytes.len() && bytes[end] & 0xc0 == 0x80 {
                end += 1;
            }
            bytes.truncate(end);
        }
        if !self.eof
            && let Err(error) = core::str::from_utf8(&bytes)
            && error.error_len().is_none()
        {
            bytes.truncate(error.valid_up_to());
        }
        let next = start + bytes.len() as u64;
        let mut screen = String::new();
        let mut clipped = false;
        'rows: for y in 0..self.terminal.rows() {
            let mut row = String::new();
            for x in 0..self.terminal.cols() {
                let cell = self.terminal.view_cell(x, y, 0);
                if cell.width == 0 {
                    continue;
                }
                row.push(cell.ch);
                for c in cell.combining {
                    if c != '\0' {
                        row.push(c);
                    }
                }
            }
            let row = row.trim_end();
            if screen.len() + row.len() + 1 > 65536 {
                clipped = true;
                break 'rows;
            }
            screen.push_str(row);
            screen.push('\n');
        }
        let mut value = self.info();
        value.set(
            "output",
            Value::from(String::from_utf8_lossy(&bytes).into_owned()),
        );
        value.set("cursor", Value::from(next));
        value.set("next_cursor", Value::from(next));
        value.set("first_cursor", Value::from(first));
        value.set("truncated", Value::Bool(cursor < first));
        value.set("has_more", Value::Bool(next < self.output_end));
        value.set("screen", Value::from(screen));
        value.set("screen_truncated", Value::Bool(clipped));
        Ok(value)
    }
}

struct Pool {
    sessions: Vec<Session>,
    connections: Vec<(String, Rc<Connection>)>,
    next: u64,
    completed: VecDeque<(String, i32)>,
    finished: Vec<(String, i32)>,
    reaping: Vec<(i32, u64)>,
}

fn bounded(
    request: &Value,
    key: &str,
    default: usize,
    min: usize,
    max: usize,
) -> Result<usize, String> {
    match request.get(key) {
        None => Ok(default),
        Some(value) => value
            .as_u64()
            .filter(|&n| n >= min as u64 && n <= max as u64)
            .map(|n| n as usize)
            .ok_or_else(|| format!("{key} must be {min}..{max}")),
    }
}

fn environment(request: &Value) -> Result<Vec<CString>, String> {
    match request.get("_env").and_then(Value::as_array) {
        None => Ok(os::environment()),
        Some(entries) => entries
            .iter()
            .map(|v| broker::unhex(v.as_str().ok_or("invalid process environment")?))
            .collect(),
    }
}

struct WorkingDirectory(broker::Fd);
impl WorkingDirectory {
    fn enter(path: &CStr) -> Result<Self, String> {
        let saved = broker::Fd(unsafe { os::open(c".".as_ptr(), 0o2000000 | 0o200000) });
        if saved.0 < 0 || unsafe { os::chdir(path.as_ptr()) } < 0 {
            return Err("cannot enter session working directory".into());
        }
        Ok(Self(saved))
    }
}
impl Drop for WorkingDirectory {
    fn drop(&mut self) {
        unsafe {
            os::fchdir(self.0.0);
        }
    }
}

impl Pool {
    fn new() -> Self {
        Self {
            sessions: Vec::new(),
            connections: Vec::new(),
            next: 1,
            completed: VecDeque::new(),
            finished: Vec::new(),
            reaping: Vec::new(),
        }
    }

    fn connection(&mut self, env: &[CString]) -> Result<Rc<Connection>, String> {
        let display = env
            .iter()
            .find_map(|e| e.to_bytes().strip_prefix(b"DISPLAY="));
        let key = display
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .unwrap_or_default();
        if let Some((_, connection)) = self.connections.iter().find(|(name, _)| *name == key) {
            return Ok(connection.clone());
        }
        let name = display
            .map(CString::new)
            .transpose()
            .map_err(|_| "invalid display name")?;
        let connection = Connection::new_for_display(name.as_deref()).map_err(String::from)?;
        self.connections.push((key, connection.clone()));
        Ok(connection)
    }

    fn selector(&self, request: &Value) -> Result<usize, String> {
        let selector = request
            .get("session")
            .and_then(Value::as_str)
            .ok_or("session is required")?;
        let mut matches = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| s.uri == selector || s.name == selector);
        let first = matches
            .next()
            .map(|(i, _)| i)
            .ok_or_else(|| format!("session not found: {selector}"))?;
        if matches.next().is_some() {
            return Err(format!(
                "ambiguous session name: {selector}; use the complete ztty:// address"
            ));
        }
        Ok(first)
    }

    fn create(&mut self, request: &Value) -> Result<Value, String> {
        if self.sessions.len() >= 128 {
            return Err("session limit reached (128); close unused sessions".into());
        }
        let origin = request
            .get("origin")
            .and_then(Value::as_str)
            .unwrap_or("local");
        let valid_origin = origin == "local"
            || origin == "mcp/stdio"
            || origin.strip_prefix("mcp/http/").is_some_and(|a| {
                !a.is_empty()
                    && a.len() < 256
                    && a.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b".:-_[]%".contains(&b))
            });
        if !valid_origin {
            return Err("invalid session origin".into());
        }
        let default = format!("session-{}", self.next);
        self.next += 1;
        let name = match request.get("name") {
            None => default.as_str(),
            Some(value) => value.as_str().ok_or("name must be a string")?,
        };
        if name.is_empty()
            || name.len() > 64
            || name == "."
            || name == ".."
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(
                "session names use 1..64 ASCII letters, digits, dot, underscore or hyphen".into(),
            );
        }
        let uri = format!("ztty://{origin}/{name}");
        if self.sessions.iter().any(|s| s.uri == uri) {
            return Err(format!("session already exists: {uri}"));
        }
        let cols = bounded(request, "cols", 120, 2, 512)?;
        let rows = bounded(request, "rows", 32, 2, 256)?;
        let history = bounded(request, "history", 2000, 0, 100000)?;
        let mut env = environment(request)?;
        let command: Vec<CString> =
            if let Some(args) = request.get("_argv").and_then(Value::as_array) {
                args.iter()
                    .map(|s| broker::unhex(s.as_str().ok_or("invalid command argument")?))
                    .collect::<Result<_, _>>()?
            } else if let Some(args) = request.get("command") {
                args.as_array()
                    .ok_or("command must be an argv array")?
                    .iter()
                    .map(|s| {
                        CString::new(s.as_str().ok_or("command arguments must be strings")?)
                            .map_err(|_| String::from("command argument contains NUL"))
                    })
                    .collect::<Result<_, _>>()?
            } else {
                alloc::vec![
                    CString::new(
                        env.iter()
                            .find_map(|e| e.to_bytes().strip_prefix(b"SHELL="))
                            .filter(|s| !s.is_empty())
                            .unwrap_or(b"/bin/sh")
                    )
                    .map_err(|_| "invalid shell")?
                ]
            };
        if command.is_empty() || command[0].is_empty() {
            return Err("command must contain an executable".into());
        }
        let mut cwd = if let Some(path) = request.get("cwd") {
            CString::new(path.as_str().ok_or("cwd must be a string")?)
                .map_err(|_| String::from("cwd contains NUL"))?
        } else if let Some(path) = request.get("_cwd").and_then(Value::as_str) {
            broker::unhex(path)?
        } else {
            c".".into()
        };
        if !cwd.to_bytes().starts_with(b"/")
            && let Some(base) = request.get("_cwd").and_then(Value::as_str)
        {
            let base = broker::unhex(base)?;
            let mut joined = base.to_bytes().to_vec();
            joined.push(b'/');
            joined.extend_from_slice(cwd.to_bytes());
            cwd = CString::new(joined).map_err(|_| "invalid working directory")?;
        }
        let appearance = if let Some(value) = request.get("_appearance") {
            Appearance::from_json(value).map_err(String::from)?
        } else {
            Appearance::load(None)?
        };
        let headless = request
            .get("headless")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let ui = if headless {
            None
        } else {
            let c = self.connection(&env)?;
            Some(Ui::new(
                Window::new_shared(cols, rows, &appearance, &c).map_err(String::from)?,
            ))
        };
        let directory = WorkingDirectory::enter(&cwd)?;
        env.retain(|e| !e.to_bytes().starts_with(b"PWD="));
        let mut actual = [0u8; 4096];
        if unsafe { os::getcwd(actual.as_mut_ptr().cast(), actual.len()) }.is_null() {
            return Err("cannot resolve session directory".into());
        }
        let len = actual
            .iter()
            .position(|&b| b == 0)
            .ok_or("working directory is too long")?;
        let mut pwd = b"PWD=".to_vec();
        pwd.extend_from_slice(&actual[..len]);
        env.push(CString::new(pwd).map_err(|_| "invalid working directory")?);
        let (program, env) = os::child_setup_with_env(&command[0], env).map_err(String::from)?;
        let argv: Vec<&CStr> = command.iter().map(CString::as_c_str).collect();
        let env: Vec<&CStr> = env.iter().map(CString::as_c_str).collect();
        let close: Vec<i32> = self.connections.iter().map(|(_, c)| c.fd).collect();
        let pty = unsafe {
            zpty::Pty::spawn(
                &program,
                &argv,
                &env,
                zpty::Size::new(cols as u16, rows as u16),
                &close,
            )
        }
        .map_err(|e| format!("cannot create PTY: {e}"))?;
        drop(directory);
        let terminal = Terminal::with_theme(cols, rows, history, appearance.theme);
        let session = Session {
            uri,
            name: name.into(),
            origin: origin.into(),
            appearance,
            pty,
            terminal,
            queue: Queue::new(),
            ui,
            persistent: origin != "local",
            eof: false,
            output: VecDeque::new(),
            output_end: 0,
        };
        let result = session.info();
        self.sessions.push(session);
        Ok(result)
    }

    fn retire(&mut self, index: usize) -> Value {
        let session = self.sessions.remove(index);
        let mut result = session.info();
        result.set("closed", Value::Bool(true));
        result.set("running", Value::Bool(false));
        result.set("attached", Value::Bool(false));
        let status = session.status().unwrap_or(0);
        self.finished.push((session.uri.clone(), status));
        self.completed.push_back((session.uri.clone(), status));
        while self.completed.len() > 128 {
            self.completed.pop_front();
        }
        if session.pty.status().is_none() {
            let _ = session.pty.hangup();
            self.reaping.push((session.pty.pid(), os::milliseconds()));
        }
        drop(session);
        result
    }

    fn handle(&mut self, request: &Value) -> Result<Reply, String> {
        let op = request
            .get("op")
            .and_then(Value::as_str)
            .ok_or("missing operation")?;
        if op == "create" {
            return self.create(request).map(Reply::Value);
        }
        if op == "list" {
            return Ok(Reply::Value(json::object([
                (
                    "sessions",
                    Value::Array(self.sessions.iter().map(Session::info).collect()),
                ),
                ("broker_pid", Value::from(unsafe { os::getpid() })),
            ])));
        }
        if op == "wait" {
            if let Some(name) = request.get("session").and_then(Value::as_str)
                && !self.sessions.iter().any(|s| s.uri == name)
                && let Some((_, status)) = self.completed.iter().rev().find(|(uri, _)| uri == name)
            {
                return Ok(Reply::Value(json::object([(
                    "exit_code",
                    Value::from(*status),
                )])));
            }
        }
        let index = self.selector(request)?;
        match op {
            "read" => self.sessions[index].read(request).map(Reply::Value),
            "write" => {
                let session = &mut self.sessions[index];
                if session.eof || session.status().is_some() {
                    return Err("session has exited".into());
                }
                let data = request
                    .get("data")
                    .and_then(Value::as_str)
                    .ok_or("data must be a string")?;
                if data.len() > 65536 {
                    return Err("write data exceeds 65536 bytes".into());
                }
                if session
                    .queue
                    .bytes
                    .len()
                    .saturating_sub(session.queue.offset)
                    + data.len()
                    > 1024 * 1024
                {
                    return Err("session input queue is full".into());
                }
                session.queue.push(data.as_bytes());
                session.queue.flush(&session.pty);
                Ok(Reply::Value(json::object([
                    ("written", Value::from(data.len())),
                    ("uri", Value::from(session.uri.clone())),
                ])))
            }
            "resize" => {
                let cols = bounded(request, "cols", 120, 2, 512)?;
                let rows = bounded(request, "rows", 32, 2, 256)?;
                let session = &mut self.sessions[index];
                session
                    .pty
                    .resize(zpty::Size::new(cols as u16, rows as u16))
                    .map_err(|e| format!("resize failed: {e}"))?;
                session.terminal.resize(cols, rows);
                if let Some(ui) = &mut session.ui {
                    ui.window.resize_grid(cols, rows);
                    ui.force = true;
                    ui.history = ui.history.min(session.terminal.history_len());
                }
                Ok(Reply::Value(session.info()))
            }
            "attach" => {
                if let Some(ui) = &mut self.sessions[index].ui {
                    ui.window.raise();
                    return Ok(Reply::Value(self.sessions[index].info()));
                }
                let env = environment(request)?;
                let connection = self.connection(&env)?;
                let session = &mut self.sessions[index];
                if let Some(value) = request.get("_appearance") {
                    session.appearance = Appearance::from_json(value).map_err(String::from)?;
                }
                let window = Window::new_shared(
                    session.terminal.cols(),
                    session.terminal.rows(),
                    &session.appearance,
                    &connection,
                )
                .map_err(String::from)?;
                session.ui = Some(Ui::new(window));
                Ok(Reply::Value(session.info()))
            }
            "wait" => {
                let session = &self.sessions[index];
                if session.ui.is_none() {
                    return Ok(Reply::Value(json::object([(
                        "exit_code",
                        Value::from(session.status().unwrap_or(0)),
                    )])));
                }
                Ok(Reply::Wait(session.uri.clone()))
            }
            "close" => Ok(Reply::Value(self.retire(index))),
            _ => Err(format!("unknown session operation: {op}")),
        }
    }

    fn events(&mut self, openers: &mut os::Openers, text: &mut Vec<u8>, encoded: &mut Vec<u8>) {
        for (_, connection) in &self.connections {
            for _ in 0..512 {
                if unsafe { x11::XPending(connection.display) } == 0 {
                    break;
                }
                let mut event = x11::Event { pad: [0; 24] };
                unsafe {
                    x11::XNextEvent(connection.display, &mut event);
                }
                if unsafe { x11::XFilterEvent(&mut event, 0) } != 0 {
                    continue;
                }
                let id = event.window();
                if let Some(session) = self.sessions.iter_mut().find(|s| {
                    s.ui.as_ref().is_some_and(|ui| {
                        ui.window.id == id && ui.window.display == connection.display
                    })
                }) {
                    let ui = session.ui.as_mut().unwrap();
                    if ui
                        .event(
                            &mut event,
                            &mut session.terminal,
                            &session.pty,
                            &mut session.queue,
                            openers,
                            text,
                            encoded,
                        )
                        .is_err()
                    {
                        ui.open = false;
                    }
                }
            }
        }
    }

    fn tick(&mut self, buffer: &mut [u8]) -> Option<u64> {
        let mut deadline = None;
        for session in &mut self.sessions {
            session.pump(buffer);
            if let Some(ui) = &mut session.ui
                && let Some(next) = ui.paint(&mut session.terminal, &session.appearance)
            {
                deadline = Some(deadline.map_or(next, |old: u64| old.min(next)));
            }
            if session.eof && session.status().is_none() {
                let next = os::milliseconds() + 20;
                deadline = Some(deadline.map_or(next, |old| old.min(next)));
            }
        }
        for i in (0..self.sessions.len()).rev() {
            let closed = self.sessions[i].ui.as_ref().is_some_and(|ui| !ui.open);
            let ended = self.sessions[i].eof && self.sessions[i].status().is_some();
            if !self.sessions[i].persistent && (closed || ended) {
                self.retire(i);
            } else if closed {
                let session = &mut self.sessions[i];
                session.ui = None;
                self.finished
                    .push((session.uri.clone(), session.status().unwrap_or(0)));
            }
        }
        self.connections
            .retain(|(_, connection)| Rc::strong_count(connection) > 1);
        let now = os::milliseconds();
        self.reaping.retain(|&(pid, start)| {
            let result = unsafe { os::waitpid(pid, core::ptr::null_mut(), 1) };
            if result == pid || result < 0 && os::errno() != os::EINTR {
                return false;
            }
            if now.saturating_sub(start) > 500 {
                unsafe {
                    os::kill(-pid, 9);
                }
            }
            true
        });
        if !self.reaping.is_empty() {
            let next = now + 20;
            deadline = Some(deadline.map_or(next, |old| old.min(next)));
        }
        deadline
    }
}

pub fn serve() -> Result<(), String> {
    let mut server = broker::Server::new()?;
    let mut pool = Pool::new();
    let mut openers = os::Openers::new();
    let mut read_buffer = [0u8; 16384];
    let mut text = alloc::vec![0u8;4096];
    let mut encoded = Vec::with_capacity(128);
    let mut fds = Vec::new();
    let mut idle_since = os::milliseconds();
    loop {
        pool.events(&mut openers, &mut text, &mut encoded);
        let deadline = pool.tick(&mut read_buffer);
        server.pump(|request| pool.handle(request));
        for (uri, status) in pool.finished.drain(..) {
            server.finish(&uri, status);
        }
        openers.reap();
        if !pool.sessions.is_empty() || server.has_pending() {
            idle_since = os::milliseconds();
        }
        if pool.sessions.is_empty()
            && pool.reaping.is_empty()
            && os::milliseconds().saturating_sub(idle_since) > 3000
        {
            return Ok(());
        }
        fds.clear();
        server.descriptors(&mut fds);
        for (_, connection) in &pool.connections {
            fds.push(os::PollFd {
                fd: connection.fd,
                events: os::POLLIN,
                revents: 0,
            });
        }
        for session in &pool.sessions {
            if !session.eof {
                fds.push(os::PollFd {
                    fd: session.pty.fd(),
                    events: os::POLLIN
                        | if session.queue.pending() {
                            os::POLLOUT
                        } else {
                            0
                        },
                    revents: 0,
                });
            }
        }
        let mut timeout = deadline
            .map(|end| end.saturating_sub(os::milliseconds()).min(5000) as c_int)
            .unwrap_or(5000);
        if pool.sessions.is_empty() {
            timeout = timeout.min(100);
        }
        if openers.pending() {
            timeout = timeout.min(50);
        }
        if pool
            .connections
            .iter()
            .any(|(_, c)| unsafe { x11::XPending(c.display) } != 0)
        {
            timeout = 0;
        }
        if unsafe { os::poll(fds.as_mut_ptr(), fds.len(), timeout) } < 0 && os::errno() != os::EINTR
        {
            return Err("session event loop poll failed".into());
        }
    }
}
