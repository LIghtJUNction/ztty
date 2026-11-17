//! Shared PTYs and desktop windows, driven by one poll loop.
use crate::{
    Appearance, Queue, actions,
    broker::{self, Reply},
    config_watch,
    json::{self, Value},
    key, key_modifiers,
    layout::{Axis, Layout},
    modifiers, os, paste, search, workspace_state,
    x11::{self, Connection, Selection, Window},
};
use alloc::{collections::VecDeque, ffi::CString, format, rc::Rc, string::String, vec::Vec};
use core::ffi::{CStr, c_int};
use input::{Key, KeyEvent, MouseEvent};
use zterm::{CursorShape, Terminal, input};

struct InputBuffers {
    text: Vec<u8>,
    encoded: Vec<u8>,
}

struct Search {
    query: String,
    hit: Option<search::Hit>,
    count: usize,
    ordinal: usize,
}

struct Palette {
    query: String,
    selected: usize,
}

fn pty_size(cols: usize, rows: usize, window: Option<&Window>) -> zpty::Size {
    let (pixel_width, pixel_height) = window.map_or((0, 0), |window| {
        (
            cols.saturating_mul(window.cw as usize)
                .min(u16::MAX as usize) as u16,
            rows.saturating_mul(window.ch as usize)
                .min(u16::MAX as usize) as u16,
        )
    });
    zpty::Size {
        cols: cols as u16,
        rows: rows as u16,
        pixel_width,
        pixel_height,
    }
}

struct Ui {
    window: Window,
    workspace: Option<u64>,
    visible: bool,
    read_only: bool,
    // A newly launched window owns its child; attaching only owns a view.
    owns_child: bool,
    search: Option<Search>,
    palette: Option<Palette>,
    pending_action: Option<&'static str>,
    message: Option<(String, u64)>,
    command: Option<u64>,
    attachment: u64,
    selection: Option<Selection>,
    history: usize,
    force: bool,
    open: bool,
    title: String,
    held: [u64; 256],
    intercepted: [bool; 256],
    local_buttons: [bool; 12],
    swallowed_buttons: [bool; 12],
    // X11 core motion events expose masks only for buttons 1..5. Track
    // presses actually sent to the PTY so extra-button drags keep identity.
    reported_buttons: u16,
    sync_since: Option<u64>,
    blink_deadline: Option<u64>,
    cursor_visible: bool,
    last_cursor: Option<((usize, usize), CursorShape)>,
    reset_blink: bool,
}
impl Ui {
    fn new(window: Window, attachment: u64) -> Self {
        Self {
            window,
            workspace: None,
            visible: true,
            read_only: false,
            owns_child: false,
            search: None,
            palette: None,
            pending_action: None,
            message: None,
            command: None,
            attachment,
            selection: None,
            history: 0,
            force: true,
            open: true,
            title: String::new(),
            held: [0; 256],
            intercepted: [false; 256],
            local_buttons: [false; 12],
            swallowed_buttons: [false; 12],
            reported_buttons: 0,
            sync_since: None,
            blink_deadline: None,
            cursor_visible: true,
            last_cursor: None,
            reset_blink: false,
        }
    }
    fn error(&mut self, message: String) {
        self.message = Some((
            message
                .chars()
                .filter(|c| !c.is_control())
                .take(512)
                .collect(),
            os::milliseconds() + 5000,
        ));
        self.force = true;
    }
    fn reset_input(&mut self) {
        self.held.fill(0);
        self.intercepted.fill(false);
        self.local_buttons.fill(false);
        self.swallowed_buttons.fill(false);
        self.reported_buttons = 0;
        if let Some(selection) = &mut self.selection {
            selection.dragging = false;
        }
    }
    fn cancel_input(&mut self) {
        self.window.cancel_paste();
        self.reset_input();
    }
    fn find(&mut self, terminal: &Terminal, forward: bool, restart: bool) {
        let Some(search) = &mut self.search else {
            return;
        };
        let result = search::find(
            terminal,
            &search.query,
            if restart { None } else { search.hit },
            forward,
        );
        search.hit = result.hit;
        search.count = result.count;
        search.ordinal = result.ordinal;
        if let Some(hit) = result.hit {
            let (history, row) = search::viewport(terminal, hit);
            self.history = history;
            self.selection = Some(Selection {
                start: (hit.start, row),
                end: (hit.end, row),
                dragging: false,
            });
        } else {
            self.selection = None;
        }
        self.force = true;
    }

    fn start_search(&mut self, terminal: &Terminal) {
        let query = self
            .selection
            .map(|selection| selection.text(terminal, self.history))
            .filter(|text| text.len() <= 1024 && !text.contains('\n'))
            .unwrap_or_default();
        self.search = Some(Search {
            query,
            hit: None,
            count: 0,
            ordinal: 0,
        });
        self.find(terminal, false, true);
    }
    // Keep model, PTY and scratch buffers independently borrowed; the view
    // deliberately does not own the session or its input queue.
    #[allow(clippy::too_many_arguments)]
    fn event(
        &mut self,
        event: &mut x11::Event,
        terminal: &mut Terminal,
        pty: &zpty::Pty,
        queue: &mut Queue,
        openers: &mut os::Openers,
        buffers: &mut InputBuffers,
        allow_input: bool,
    ) -> Result<(), &'static str> {
        let InputBuffers {
            text: text_buffer,
            encoded,
        } = buffers;
        match unsafe { event.kind } {
            x11::KEY_PRESS | x11::KEY_RELEASE => {
                let mut e = unsafe { event.key };
                let code = (e.keycode as usize).min(255);
                let press = e.kind == x11::KEY_PRESS;
                if !press && self.held[code] == 0 {
                    return Ok(());
                }
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
                if self.search.is_some() {
                    if !press {
                        return Ok(());
                    }
                    self.intercepted[code] = true;
                    match symbol {
                        0xff1b => {
                            self.search = None;
                            self.window.search_matches(0, Vec::new());
                            self.force = true;
                        }
                        0xff0d | 0xffc0 => self.find(terminal, mods.0 & 1 == 0, false),
                        0xff08 => {
                            self.search.as_mut().unwrap().query.pop();
                            self.find(terminal, false, true);
                        }
                        0x61 | 0x41 | 0x75 | 0x55 if mods.0 & 4 != 0 => {
                            self.search.as_mut().unwrap().query.clear();
                            self.find(terminal, false, true);
                        }
                        _ => {
                            let lookup = self.window.lookup(&mut e, text_buffer);
                            if !lookup.ignored {
                                let text =
                                    core::str::from_utf8(&text_buffer[..lookup.len]).unwrap_or("");
                                let query = &mut self.search.as_mut().unwrap().query;
                                for ch in text.chars().filter(|ch| !ch.is_control()) {
                                    if query.len() + ch.len_utf8() <= 1024 {
                                        query.push(ch);
                                    }
                                }
                                self.find(terminal, false, true);
                            }
                        }
                    }
                    return Ok(());
                }
                if let Some(palette) = &mut self.palette {
                    if !press {
                        return Ok(());
                    }
                    self.intercepted[code] = true;
                    match symbol {
                        0xff1b => self.palette = None,
                        0xff0d => {
                            self.pending_action = actions::matching(&palette.query)
                                .nth(palette.selected)
                                .map(|a| a.name);
                            self.palette = None;
                        }
                        0xff52 | 0xff54 => {
                            let count = actions::matching(&palette.query).count();
                            if count != 0 {
                                palette.selected = (palette.selected
                                    + if symbol == 0xff54 { 1 } else { count - 1 })
                                    % count;
                            }
                        }
                        0xff08 => {
                            palette.query.pop();
                            palette.selected = 0;
                        }
                        0x61 | 0x41 | 0x75 | 0x55 if mods.0 & 4 != 0 => {
                            palette.query.clear();
                            palette.selected = 0;
                        }
                        _ => {
                            let lookup = self.window.lookup(&mut e, text_buffer);
                            if !lookup.ignored {
                                for ch in core::str::from_utf8(&text_buffer[..lookup.len])
                                    .unwrap_or("")
                                    .chars()
                                    .filter(|c| !c.is_control())
                                {
                                    if palette.query.len() + ch.len_utf8() <= 128 {
                                        palette.query.push(ch.to_ascii_lowercase());
                                    }
                                }
                                palette.selected = 0;
                            }
                        }
                    }
                    self.force = true;
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
                if !allow_input {
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
                if !(1..=11).contains(&e.button) {
                    return Ok(());
                }
                let (col, row) =
                    self.window
                        .coordinates(e.x, e.y, terminal.cols(), terminal.rows());
                let mods = modifiers(e.state);
                let pressed = e.kind == x11::BUTTON_PRESS;
                let button_index = e.button as usize;
                let mask = 1u16 << button_index;
                let was_reported = self.reported_buttons & mask != 0;
                self.reported_buttons &= !mask;
                if !pressed && self.swallowed_buttons[button_index] {
                    self.swallowed_buttons[button_index] = false;
                    return Ok(());
                }
                let mut local =
                    !allow_input || mods.0 & 1 != 0 || terminal.modes().mouse_tracking == 0;
                if pressed && local {
                    self.local_buttons[button_index] = true;
                }
                if !pressed {
                    // A release belongs to the destination of its press,
                    // even when Shift changes while the button is held.
                    local =
                        !allow_input || self.local_buttons[button_index] || !was_reported && local;
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
                            self.window.copy(s.text(terminal, self.history), true);
                            self.force = true;
                        }
                    } else if pressed && e.button == 2 {
                        if allow_input {
                            self.window.paste(true);
                        }
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
                    let mouse = if pressed {
                        MouseEvent::Press {
                            button: (e.button - 1) as u8,
                            col,
                            row,
                        }
                    } else {
                        if !was_reported {
                            return Ok(());
                        }
                        MouseEvent::Release {
                            button: (e.button - 1) as u8,
                            col,
                            row,
                        }
                    };
                    encoded.clear();
                    input::encode_mouse_into(mouse, mods, &terminal.modes().input(), encoded);
                    if pressed && !(4..=7).contains(&e.button) && !encoded.is_empty() {
                        self.reported_buttons |= mask;
                    }
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
                if !allow_input {
                    return Ok(());
                }
                let button = (self.reported_buttons != 0)
                    .then(|| self.reported_buttons.trailing_zeros() as u8 - 1);
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
                // Ignore geometry queued before the latest layout request.
                // This uses Xlib sequence numbers without a server round trip.
                if e.serial < self.window.configure_serial.get() {
                    return Ok(());
                }
                let (cols, rows) = self.window.resize(e.width, e.height);
                if allow_input && (cols != terminal.cols() || rows != terminal.rows()) {
                    match pty.resize(pty_size(cols, rows, Some(&self.window))) {
                        Ok(()) => terminal.resize(cols, rows),
                        Err(error) => self.error(format!("cannot resize the PTY: {error}")),
                    }
                }
                self.history = self.history.min(terminal.history_len());
                self.selection = None;
                self.force = true;
            }
            x11::FOCUS_IN | x11::FOCUS_OUT => {
                let focused = unsafe { event.kind } == x11::FOCUS_IN;
                self.window.focus(focused);
                self.reset_blink = true;
                self.force = true;
                if !focused {
                    self.reset_input();
                }
                if allow_input {
                    queue.push(&input::encode_focus(focused, &terminal.modes().input()));
                }
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
                if let Some(data) = self.window.selection_notify(unsafe { event.selection })
                    && allow_input
                {
                    paste(data, terminal, queue);
                }
            }
            x11::PROPERTY_NOTIFY => {
                if let Some(data) = self.window.property(unsafe { event.property })
                    && allow_input
                {
                    paste(data, terminal, queue);
                }
            }
            x11::SELECTION_CLEAR => {}
            _ => {}
        }
        Ok(())
    }
    fn paint(
        &mut self,
        terminal: &mut Terminal,
        appearance: &Appearance,
        observing: bool,
    ) -> Option<u64> {
        if self
            .message
            .as_ref()
            .is_some_and(|(_, until)| *until <= os::milliseconds())
        {
            self.message = None;
            self.force = true;
        }
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
        let title = if observing {
            format!(
                "{} · observe",
                if terminal.title().is_empty() {
                    "ztty"
                } else {
                    terminal.title()
                }
            )
        } else {
            terminal.title().into()
        };
        if title != self.title {
            self.title = title;
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
        if !synchronized
            && (self.force
                || terminal.dirty()
                || self.window.cursor_changed(cursor)
                || self.window.graphics_changed(terminal))
        {
            if let Some(search) = &self.search
                && (self.force || terminal.dirty())
            {
                self.window.search_matches(
                    terminal.cols(),
                    search::highlights(terminal, &search.query, self.history),
                );
            }
            self.window
                .render(terminal, self.history, self.selection, self.force, cursor);
            if let Some(search) = &self.search {
                self.window.overlay(&format!(
                    "Find: {}   {}/{}   Enter / Shift+Enter · Esc",
                    search.query, search.ordinal, search.count
                ));
            }
            if let Some(palette) = &self.palette {
                let mut lines = alloc::vec![format!("Run: {}", palette.query)];
                let start = palette.selected.saturating_sub(2);
                for (index, action) in actions::matching(&palette.query)
                    .enumerate()
                    .skip(start)
                    .take(5)
                {
                    lines.push(format!(
                        "{} {}",
                        if index == palette.selected { ">" } else { " " },
                        action.label
                    ));
                }
                if lines.len() == 1 {
                    lines.push("  No matching action".into());
                }
                lines.push("↑/↓ select · Enter run · Esc close".into());
                self.window.overlay_lines(&lines);
            }
            if let Some((message, _)) = &self.message {
                self.window.overlay(&format!("ztty: {message}"));
            }
            self.force = false;
        }

        let deadline = match (self.sync_since.map(|s| s + 150), self.blink_deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        self.message
            .as_ref()
            .map(|(_, until)| deadline.map_or(*until, |d| d.min(*until)))
            .or(deadline)
    }
}

struct Session {
    id: u64,
    created_at: u64,
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
    input_owner: Option<String>,
    launch_env: Vec<CString>,
    initial_cwd: CString,
    config_path: Option<CString>,
    base_font: String,
}

impl Session {
    fn status(&self) -> Option<i32> {
        self.pty
            .status()
            .map(|s| s.code().unwrap_or(128 + s.signal().unwrap_or(0)))
    }
    fn info(&self) -> Value {
        json::object([
            ("id", Value::from(self.id)),
            ("created_at", Value::from(self.created_at)),
            ("uri", Value::from(self.uri.clone())),
            ("name", Value::from(self.name.clone())),
            ("origin", Value::from(self.origin.clone())),
            ("pid", Value::from(self.pty.pid())),
            ("broker_pid", Value::from(unsafe { os::getpid() })),
            ("cols", Value::from(self.terminal.cols())),
            ("rows", Value::from(self.terminal.rows())),
            ("attached", Value::Bool(self.ui.is_some())),
            (
                "read_only",
                Value::Bool(self.ui.as_ref().is_some_and(|ui| ui.read_only)),
            ),
            (
                "attachment",
                self.ui
                    .as_ref()
                    .map(|ui| Value::from(ui.attachment))
                    .unwrap_or(Value::Null),
            ),
            (
                "running",
                Value::Bool(!self.eof && self.pty.status().is_none()),
            ),
            (
                "exit_code",
                self.status().map(Value::from).unwrap_or(Value::Null),
            ),
            ("title", Value::from(self.terminal.title())),
            (
                "input_owner",
                self.input_owner
                    .as_ref()
                    .map(|s| Value::from(s.clone()))
                    .unwrap_or(Value::Null),
            ),
            (
                "workspace",
                self.ui
                    .as_ref()
                    .and_then(|ui| ui.workspace)
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            ),
            (
                "visible",
                Value::Bool(self.ui.as_ref().is_some_and(|ui| ui.visible)),
            ),
            (
                "window_id",
                self.ui
                    .as_ref()
                    .map(|ui| Value::from(ui.window.id))
                    .unwrap_or(Value::Null),
            ),
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

struct Workspace {
    window: x11::Container,
    layout: Layout,
    title: String,
}

struct Pool {
    sessions: Vec<Session>,
    workspaces: Vec<Workspace>,
    connections: Vec<(String, Rc<Connection>)>,
    next: u64,
    next_attachment: u64,
    completed: VecDeque<(String, i32)>,
    finished: Vec<(String, i32)>,
    reaping: Vec<(i32, u64)>,
    shortcuts: [bool; 256],
    mouse_shortcuts: [bool; 12],
    watcher: config_watch::Watcher,
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

fn actor(request: &Value) -> Result<&str, String> {
    match request.get("_actor") {
        None => Ok("local"),
        Some(value) => value
            .as_str()
            .filter(|s| {
                !s.is_empty()
                    && s.len() <= 128
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
            })
            .ok_or_else(|| String::from("invalid control identity")),
    }
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

fn local_directory(uri: &str) -> Option<String> {
    let (host, path) = uri.strip_prefix("file://")?.split_once('/')?;
    let mut hostname = [0u8; 256];
    let actual = if unsafe { os::gethostname(hostname.as_mut_ptr().cast(), hostname.len()) } == 0 {
        let len = hostname
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(hostname.len());
        core::str::from_utf8(&hostname[..len]).ok()?
    } else {
        ""
    };
    if !host.is_empty() && host != "localhost" && host != actual {
        return None;
    }
    let mut bytes = Vec::with_capacity(path.len() + 1);
    bytes.push(b'/');
    let mut source = path.as_bytes().iter().copied();
    while let Some(byte) = source.next() {
        let byte = if byte == b'%' {
            let high = (source.next()? as char).to_digit(16)?;
            let low = (source.next()? as char).to_digit(16)?;
            (high * 16 + low) as u8
        } else {
            byte
        };
        if byte == 0 || byte < 32 || byte == 127 {
            return None;
        }
        bytes.push(byte);
    }
    String::from_utf8(bytes).ok()
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
            workspaces: Vec::new(),
            connections: Vec::new(),
            next: 1,
            next_attachment: 1,
            completed: VecDeque::new(),
            finished: Vec::new(),
            reaping: Vec::new(),
            shortcuts: [false; 256],
            mouse_shortcuts: [false; 12],
            watcher: config_watch::Watcher::new(),
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
        if let Some(id) = request.get("session").and_then(Value::as_u64) {
            return self
                .sessions
                .iter()
                .position(|s| s.id == id)
                .ok_or_else(|| format!("session id not found: {id}"));
        }
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
        let input_owner = actor(request)?.into();
        let observe = match request.get("observe") {
            None => false,
            Some(value) => value.as_bool().ok_or("observe must be a boolean")?,
        };
        if observe && request.get("target").is_some() {
            return Err("observing launches require a standalone view".into());
        }
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
        let id = self.next;
        let default = format!("session-{id}");
        self.next += 1;
        let name = match request.get("name") {
            None => default.as_str(),
            Some(value) => value.as_str().ok_or("name must be a string")?,
        };
        if !valid_name(name) {
            return Err(
                "session names use 1..64 ASCII letters, digits, dot, underscore or hyphen".into(),
            );
        }
        let uri = format!("ztty://{origin}/{name}");
        if self
            .sessions
            .iter()
            .any(|s| s.uri == uri || (s.origin == origin && s.name == name))
        {
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
        let config_path = match request.get("_config").and_then(Value::as_str) {
            Some(path) => Some(broker::unhex(path)?),
            None => os::default_config_path(),
        };
        let config_path = config_path
            .map(|path| {
                if path.to_bytes().starts_with(b"/") {
                    Ok(path)
                } else {
                    let base = request
                        .get("_cwd")
                        .and_then(Value::as_str)
                        .ok_or("relative config requires caller directory")?;
                    let mut bytes = broker::unhex(base)?.to_bytes().to_vec();
                    bytes.push(b'/');
                    bytes.extend_from_slice(path.to_bytes());
                    CString::new(bytes).map_err(|_| String::from("invalid config path"))
                }
            })
            .transpose()?;
        let headless = request
            .get("headless")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let ui = if headless {
            None
        } else {
            let c = self.connection(&env)?;
            let attachment = self.next_attachment;
            self.next_attachment += 1;
            let window = if observe {
                Window::new_view(cols, rows, &appearance, &c, false)
            } else {
                Window::new_shared(cols, rows, &appearance, &c)
            }
            .map_err(String::from)?;
            let mut ui = Ui::new(window, attachment);
            ui.read_only = observe;
            ui.owns_child = true;
            Some(ui)
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
        let child_env: Vec<&CStr> = env.iter().map(CString::as_c_str).collect();
        let close: Vec<i32> = self.connections.iter().map(|(_, c)| c.fd).collect();
        let pty = unsafe {
            zpty::Pty::spawn(
                &program,
                &argv,
                &child_env,
                pty_size(cols, rows, ui.as_ref().map(|ui| &ui.window)),
                &close,
            )
        }
        .map_err(|e| format!("cannot create PTY: {e}"))?;
        drop(directory);
        let mut terminal = Terminal::with_theme(cols, rows, history, appearance.theme);
        if let Some(ui) = &ui {
            terminal.set_cell_size(ui.window.cw as u32, ui.window.ch as u32);
        }
        let session = Session {
            id,
            created_at: os::unix_milliseconds(),
            uri,
            name: name.into(),
            origin: origin.into(),
            base_font: appearance.font.clone(),
            appearance,
            pty,
            terminal,
            queue: Queue::new(),
            ui,
            persistent: origin != "local",
            eof: false,
            output: VecDeque::new(),
            output_end: 0,
            input_owner: Some(input_owner),
            launch_env: env,
            initial_cwd: cwd,
            config_path,
        };
        if let Some(path) = &session.config_path {
            self.watcher.watch(path);
        }
        self.sessions.push(session);
        if let Some(target) = request.get("target").and_then(Value::as_str) {
            let target_request = json::object([("session", Value::from(target))]);
            let result = self.selector(&target_request).and_then(|target_index| {
                self.group(
                    self.sessions[target_index].id,
                    id,
                    request
                        .get("layout")
                        .and_then(Value::as_str)
                        .unwrap_or("tab"),
                )
            });
            if let Err(error) = result {
                self.retire(self.sessions.len() - 1);
                return Err(error);
            }
        }
        Ok(self.sessions.last().expect("created session").info())
    }

    fn retire(&mut self, index: usize) -> Value {
        let session = self.sessions.remove(index);
        self.watcher.retain(|path| {
            self.sessions
                .iter()
                .any(|s| s.config_path.as_deref() == Some(path))
        });
        let mut result = session.info();
        result.set("closed", Value::Bool(true));
        result.set("running", Value::Bool(false));
        result.set("attached", Value::Bool(false));
        let status = session.status().unwrap_or(0);
        if let Some(ui) = &session.ui {
            let token = format!("attachment:{}", ui.attachment);
            self.finished.push((token.clone(), status));
            self.completed.push_back((token, status));
        }
        while self.completed.len() > 128 {
            self.completed.pop_front();
        }
        if session.pty.status().is_none() {
            let _ = session.pty.hangup();
            self.reaping.push((session.pty.pid(), os::milliseconds()));
        }
        drop(session);
        self.remove_from_layout(result.get("id").and_then(Value::as_u64).unwrap_or(0));
        result
    }

    fn workspace(&self, session: u64) -> Option<usize> {
        self.workspaces
            .iter()
            .position(|workspace| workspace.layout.contains(session))
    }

    fn remove_from_layout(&mut self, id: u64) {
        if let Some(index) = self.workspace(id) {
            self.workspaces[index].layout.remove(id);
            if self.workspaces[index].layout.tabs.is_empty() {
                self.workspaces.remove(index);
            } else {
                let _ = self.arrange(index, false);
            }
        }
    }

    fn ensure_workspace(&mut self, id: u64) -> Result<usize, String> {
        if let Some(index) = self.workspace(id) {
            return Ok(index);
        }
        let session = self
            .sessions
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or("session not found")?;
        let ui = session
            .ui
            .as_mut()
            .ok_or("attach the session before arranging its window")?;
        let window = x11::Container::new(&ui.window);
        ui.window.parent(Some(window.id));
        ui.workspace = Some(window.id);
        self.workspaces.push(Workspace {
            window,
            layout: Layout::new(id),
            title: String::new(),
        });
        Ok(self.workspaces.len() - 1)
    }

    fn arrange(&mut self, index: usize, focus: bool) -> Result<(), String> {
        let workspace = &self.workspaces[index];
        let placements = workspace
            .layout
            .placements(workspace.window.width, workspace.window.height);
        let members = workspace.layout.panes();
        let active = workspace.layout.focused;
        let mut failure = None;
        for session in &mut self.sessions {
            if !members.contains(&session.id) {
                continue;
            }
            let Some(ui) = &mut session.ui else {
                continue;
            };
            if !ui.open || ui.window.id == 0 {
                continue;
            }
            let placement = placements.iter().find(|(id, _)| *id == session.id);
            if let Some((_, rect)) = placement {
                let (cols, rows) = ui.window.place(*rect);
                if session.input_owner.as_deref() == Some("local")
                    && !ui.read_only
                    && (cols != session.terminal.cols() || rows != session.terminal.rows())
                {
                    let resized = if session.eof {
                        true
                    } else {
                        match session.pty.resize(pty_size(cols, rows, Some(&ui.window))) {
                            Ok(()) => true,
                            Err(error) => {
                                if failure.is_none() {
                                    failure = Some(format!("cannot resize split PTY: {error}"));
                                }
                                false
                            }
                        }
                    };
                    // Keep the model at the accepted PTY size after a failed
                    // ioctl so the next arrange retries instead of skipping it.
                    if resized {
                        session.terminal.resize(cols, rows);
                        ui.history = ui.history.min(session.terminal.history_len());
                        ui.selection = None;
                    }
                }
                ui.window.visible(true);
                ui.force = true;
            } else {
                ui.window.visible(false);
                ui.reset_input();
                ui.blink_deadline = None;
            }
            ui.visible = placement.is_some();
        }
        if focus {
            self.workspaces[index].window.raise();
            if let Some(ui) = self
                .sessions
                .iter()
                .find(|s| s.id == active)
                .and_then(|s| s.ui.as_ref())
            {
                ui.window.raise();
            }
        }
        failure.map_or(Ok(()), Err)
    }

    fn group(&mut self, target: u64, moved: u64, mode: &str) -> Result<(), String> {
        if target == moved {
            return Err("cannot put a session beside itself".into());
        }
        let axis = match mode {
            "tab" => None,
            "right" => Some(Axis::Vertical),
            "down" => Some(Axis::Horizontal),
            _ => return Err("layout must be tab, right or down".into()),
        };
        let a = self
            .sessions
            .iter()
            .find(|s| s.id == target)
            .and_then(|s| s.ui.as_ref())
            .ok_or("target has no window")?;
        let b = self
            .sessions
            .iter()
            .find(|s| s.id == moved)
            .and_then(|s| s.ui.as_ref())
            .ok_or("session has no window")?;
        if a.window.display != b.window.display {
            return Err("sessions on different displays cannot share a window".into());
        }
        if self
            .workspace(target)
            .is_some_and(|i| self.workspaces[i].layout.contains(moved))
        {
            return Err("session already belongs to this workspace".into());
        }
        let index = self.ensure_workspace(target)?;
        let parent = self.workspaces[index].window.id;
        let mut layout = self.workspaces[index].layout.clone();
        layout.focus(target);
        if let Some(axis) = axis {
            layout.split(moved, axis)?;
        } else {
            layout.add_tab(moved)?;
        }
        // Reparent before destroying an emptied old container. X11 destroys a
        // parent's children, so reversing these operations would kill the view.
        let ui = self
            .sessions
            .iter_mut()
            .find(|s| s.id == moved)
            .and_then(|s| s.ui.as_mut())
            .unwrap();
        ui.window.parent(Some(parent));
        ui.workspace = Some(parent);
        self.remove_from_layout(moved);
        let index = self
            .workspaces
            .iter()
            .position(|w| w.window.id == parent)
            .unwrap();
        self.workspaces[index].layout = layout;
        self.arrange(index, true)
    }

    fn popout(&mut self, id: u64) -> Result<(), String> {
        let session = self
            .sessions
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or("session not found")?;
        let ui = session.ui.as_mut().ok_or("session has no window")?;
        ui.window.parent(None);
        ui.window.visible(true);
        ui.window.raise();
        ui.workspace = None;
        ui.visible = true;
        ui.force = true;
        self.remove_from_layout(id);
        Ok(())
    }

    fn detach(&mut self, index: usize) -> Value {
        let session = &mut self.sessions[index];
        session.persistent = true;
        let id = session.id;
        if let Some(ui) = session.ui.take() {
            let token = format!("attachment:{}", ui.attachment);
            let status = session.status().unwrap_or(0);
            self.finished.push((token.clone(), status));
            self.completed.push_back((token, status));
            while self.completed.len() > 128 {
                self.completed.pop_front();
            }
            drop(ui);
        }
        self.remove_from_layout(id);
        self.sessions[index].info()
    }

    fn related(&mut self, index: usize, mode: &str) -> Result<Value, String> {
        let session = &self.sessions[index];
        let mut request = json::object([
            ("op", Value::from("create")),
            ("origin", Value::from("local")),
            ("headless", Value::Bool(false)),
            ("cols", Value::from(session.terminal.cols())),
            ("rows", Value::from(session.terminal.rows())),
            ("_appearance", session.appearance.to_json()),
            (
                "_cwd",
                Value::from(broker::hex(session.initial_cwd.to_bytes())),
            ),
            (
                "_env",
                Value::Array(
                    session
                        .launch_env
                        .iter()
                        .map(|e| Value::from(broker::hex(e.to_bytes())))
                        .collect(),
                ),
            ),
        ]);
        // OSC 7 is parsed as data. Only local absolute file URLs can influence
        // a new local shell's cwd; an SSH host is never interpreted as local.
        if let Some(cwd) = local_directory(session.terminal.working_directory()) {
            request.set("cwd", Value::from(cwd));
        }
        if let Some(path) = &session.config_path {
            request.set("_config", Value::from(broker::hex(path.to_bytes())));
        }
        if mode != "window" {
            request.set("target", Value::from(session.uri.clone()));
            request.set("layout", Value::from(mode));
        }
        self.create(&request)
    }

    fn save_layout(&self, index: usize) -> Result<Value, String> {
        let session = &self.sessions[index];
        let ui = session
            .ui
            .as_ref()
            .filter(|ui| ui.open && ui.window.id != 0)
            .ok_or("session has no open view")?;
        let single;
        let (layout, width, height) = if let Some(index) = self.workspace(session.id) {
            let workspace = &self.workspaces[index];
            (
                &workspace.layout,
                workspace.window.width,
                workspace.window.height,
            )
        } else {
            single = Layout::new(session.id);
            (&single, ui.window.width, ui.window.height)
        };
        let broker_pid = unsafe { os::getpid() } as u32;
        workspace_state::export(layout, width, height, |id| {
            self.sessions
                .iter()
                .find(|s| s.id == id)
                .map(|s| workspace_state::SessionIdentity {
                    uri: s.uri.clone(),
                    id: s.id,
                    created_at: s.created_at,
                    broker_pid,
                })
        })
        .map_err(String::from)
    }

    fn restore_layout(&mut self, value: &Value) -> Result<Value, String> {
        let broker_pid = unsafe { os::getpid() } as u32;
        let snapshot = workspace_state::import(value, |uri, id, created_at, saved_broker| {
            self.sessions
                .iter()
                .find(|s| {
                    s.uri == uri
                        && s.id == id
                        && s.created_at == created_at
                        && saved_broker == broker_pid
                })
                .map(|s| s.id)
        })
        .map_err(String::from)?;
        let members = snapshot.layout.panes();
        let focused = snapshot.layout.focused;
        let active = self
            .sessions
            .iter()
            .position(|s| s.id == focused)
            .ok_or("focused session is unavailable")?;
        let display = self.sessions[active]
            .ui
            .as_ref()
            .ok_or("layout sessions must have attached views")?
            .window
            .display;
        // All schema, identity and display checks finish before changing any
        // live layout. A snapshot never starts or resurrects child processes.
        for id in &members {
            let ui = self
                .sessions
                .iter()
                .find(|s| s.id == *id)
                .and_then(|s| s.ui.as_ref())
                .filter(|ui| ui.open && ui.window.id != 0)
                .ok_or("layout sessions must have attached views")?;
            if ui.window.display != display {
                return Err("layout sessions must use the same display".into());
            }
        }
        let mut window = x11::Container::new(&self.sessions[active].ui.as_ref().unwrap().window);
        window.resize(snapshot.width, snapshot.height);
        let parent = window.id;
        for session in &mut self.sessions {
            if members.contains(&session.id) {
                let ui = session.ui.as_mut().unwrap();
                ui.window.parent(Some(parent));
                ui.workspace = Some(parent);
            }
        }
        for workspace in &mut self.workspaces {
            for id in &members {
                workspace.layout.remove(*id);
            }
        }
        self.workspaces.retain(|w| !w.layout.tabs.is_empty());
        // Own the new parent before any fallible resize. Dropping it on an
        // error would destroy the views we have just reparented into it.
        self.workspaces.push(Workspace {
            window,
            layout: snapshot.layout,
            title: String::new(),
        });
        let restored = self.workspaces.len() - 1;
        let mut failure = None;
        for index in 0..self.workspaces.len() {
            if let Err(error) = self.arrange(index, index == restored)
                && failure.is_none()
            {
                failure = Some(error);
            }
        }
        failure.map_or_else(|| Ok(self.sessions[active].info()), Err)
    }

    fn action(&mut self, index: usize, action: &str) -> Result<Value, String> {
        let id = self.sessions[index].id;
        match action {
            "new-tab" => return self.related(index, "tab"),
            "split-right" => return self.related(index, "right"),
            "split-down" => return self.related(index, "down"),
            "new-window" => return self.related(index, "window"),
            "move-window" => self.popout(id)?,
            "detach" => return Ok(self.detach(index)),
            "close" => {
                if let Some(ui) = &mut self.sessions[index].ui {
                    ui.open = false;
                }
            }
            "search" => {
                let session = &mut self.sessions[index];
                if let Some(ui) = &mut session.ui {
                    ui.palette = None;
                    ui.start_search(&session.terminal);
                }
            }
            "palette" => {
                if let Some(ui) = &mut self.sessions[index].ui {
                    ui.search = None;
                    ui.window.search_matches(0, Vec::new());
                    ui.palette = Some(Palette {
                        query: String::new(),
                        selected: 0,
                    });
                    ui.force = true;
                }
            }
            "none" => {}
            "copy" => {
                let session = &mut self.sessions[index];
                if let Some(ui) = &mut session.ui
                    && let Some(selection) = ui.selection
                {
                    ui.window
                        .copy(selection.text(&session.terminal, ui.history), false);
                }
            }
            "paste" | "paste-primary" => {
                let session = &mut self.sessions[index];
                if session.input_owner.as_deref() != Some("local")
                    || session.ui.as_ref().is_some_and(|ui| ui.read_only)
                {
                    return Err("observing views cannot paste; take input control first".into());
                }
                if let Some(ui) = &mut session.ui {
                    ui.window.paste(action == "paste-primary");
                }
            }
            "scroll-up" | "scroll-down" => {
                let session = &mut self.sessions[index];
                if let Some(ui) = &mut session.ui {
                    let step = session.terminal.rows().saturating_sub(1).max(1);
                    ui.history = if action == "scroll-up" {
                        ui.history
                            .saturating_add(step)
                            .min(session.terminal.history_len())
                    } else {
                        ui.history.saturating_sub(step)
                    };
                    ui.selection = None;
                    ui.force = true;
                }
            }
            "previous-command" | "next-command" => {
                let session = &mut self.sessions[index];
                let ui = session.ui.as_mut().ok_or("session has no window")?;
                let command = if action == "previous-command" {
                    session.terminal.commands().rev().find(|c| {
                        ui.command.is_none_or(|id| c.id < id)
                            && session.terminal.command_scroll_offset(c.id).is_some()
                    })
                } else {
                    session.terminal.commands().find(|c| {
                        ui.command.is_some_and(|id| c.id > id)
                            && session.terminal.command_scroll_offset(c.id).is_some()
                    })
                };
                let command = command
                    .ok_or("no shell command in that direction; enable shell integration")?;
                let offset = session
                    .terminal
                    .command_scroll_offset(command.id)
                    .ok_or("command has left the retained history")?;
                ui.command = Some(command.id);
                ui.history = offset;
                ui.selection = None;
                ui.force = true;
            }
            "copy-command-output" => {
                let session = &mut self.sessions[index];
                let ui = session.ui.as_mut().ok_or("session has no window")?;
                let id = ui
                    .command
                    .or_else(|| {
                        session
                            .terminal
                            .commands()
                            .rev()
                            .find(|c| c.output.is_some())
                            .map(|c| c.id)
                    })
                    .ok_or("no command output; enable shell integration")?;
                let output = session
                    .terminal
                    .command_output(id, 65536)
                    .ok_or("command output is unavailable")?;
                if output.truncated {
                    return Err("command output is truncated; use the bounded command-output API to inspect it explicitly".into());
                }
                ui.window.copy(output.text, false);
            }
            "reload" => return self.reload(index),
            "font-increase" | "font-decrease" | "font-reset" => {
                let session = &self.sessions[index];
                let mut next = session.appearance.clone();
                next.font = if action == "font-reset" {
                    session.base_font.clone()
                } else {
                    let mut size = 11.0f64;
                    let mut name = String::new();
                    for part in next.font.split(':') {
                        if let Some(value) = part.strip_prefix("size=") {
                            size = value
                                .parse::<f64>()
                                .ok()
                                .filter(|v| v.is_finite() && *v > 0.0)
                                .ok_or("invalid font size")?;
                        } else {
                            if !name.is_empty() {
                                name.push(':');
                            }
                            name.push_str(part);
                        }
                    }
                    format!(
                        "{name}:size={}",
                        (size + if action == "font-increase" { 1.0 } else { -1.0 })
                            .clamp(4.0, 128.0)
                    )
                };
                return self.apply_appearance(index, next, false);
            }
            "take-control" => {
                let request = json::object([
                    ("action", Value::from("acquire")),
                    ("takeover", Value::Bool(true)),
                ]);
                return self.control(index, &request);
            }
            "next-tab" | "previous-tab" | "next-pane" | "previous-pane" | "swap-next"
            | "swap-previous" | "maximize" | "grow" | "shrink" => {
                let workspace = self
                    .workspace(id)
                    .ok_or("this window has no tabs or splits")?;
                let layout = &mut self.workspaces[workspace].layout;
                layout.focus(id);
                match action {
                    "next-tab" => layout.cycle_tab(true),
                    "previous-tab" => layout.cycle_tab(false),
                    "next-pane" => layout.cycle_pane(true, false),
                    "previous-pane" => layout.cycle_pane(false, false),
                    "swap-next" => layout.cycle_pane(true, true),
                    "swap-previous" => layout.cycle_pane(false, true),
                    "maximize" => layout.maximized = !layout.maximized,
                    "grow" => {
                        layout.adjust(50);
                    }
                    "shrink" => {
                        layout.adjust(-50);
                    }
                    _ => unreachable!(),
                }
                self.arrange(workspace, true)?;
            }
            _ => return Err(format!("unknown window action: {action}")),
        }
        Ok(self.sessions[index].info())
    }

    fn reload(&mut self, index: usize) -> Result<Value, String> {
        let appearance = Appearance::load(self.sessions[index].config_path.as_deref())?;
        self.apply_appearance(index, appearance, true)
    }

    fn apply_appearance(
        &mut self,
        index: usize,
        appearance: Appearance,
        reset_font: bool,
    ) -> Result<Value, String> {
        let session = &mut self.sessions[index];
        if let Some(ui) = &mut session.ui {
            let allow_resize = session.input_owner.as_deref() == Some("local") && !ui.read_only;
            let (cols, rows) = ui.window.reconfigure(&appearance, |cols, rows, cw, ch| {
                if allow_resize && !session.eof {
                    session
                        .pty
                        .resize(zpty::Size {
                            cols: cols as u16,
                            rows: rows as u16,
                            pixel_width: (cols * cw as usize).min(u16::MAX as usize) as u16,
                            pixel_height: (rows * ch as usize).min(u16::MAX as usize) as u16,
                        })
                        .map_err(|e| format!("cannot resize reconfigured PTY: {e}"))?;
                }
                Ok(())
            })?;
            session
                .terminal
                .set_cell_size(ui.window.cw as u32, ui.window.ch as u32);
            if allow_resize {
                session.terminal.resize(cols, rows);
            }
            ui.history = ui.history.min(session.terminal.history_len());
            ui.selection = None;
            ui.last_cursor = None;
            ui.force = true;
            ui.reset_blink = true;
            ui.message = None;
        }
        session.terminal.set_theme(appearance.theme);
        if reset_font {
            session.base_font = appearance.font.clone();
        }
        session.appearance = appearance;
        let id = session.id;
        if let Some(workspace) = self.workspace(id) {
            self.arrange(workspace, false)?;
        }
        Ok(self.sessions[index].info())
    }

    fn control(&mut self, index: usize, request: &Value) -> Result<Value, String> {
        let actor = actor(request)?;
        let session = &mut self.sessions[index];
        let action = request
            .get("action")
            .and_then(Value::as_str)
            .ok_or("control action is required")?;
        match action {
            "acquire" => {
                if session
                    .input_owner
                    .as_deref()
                    .is_some_and(|owner| owner != actor)
                    && request.get("takeover").and_then(Value::as_bool) != Some(true)
                {
                    return Err(
                        "session already has an input owner; explicit takeover is required".into(),
                    );
                }
                if session.input_owner.as_deref() != Some(actor) {
                    session.queue.bytes.clear();
                    session.queue.offset = 0;
                    if let Some(ui) = &mut session.ui {
                        ui.cancel_input();
                    }
                }
                session.input_owner = Some(actor.into());
                if actor == "local"
                    && let Some(ui) = &mut session.ui
                {
                    ui.read_only = false;
                }
            }
            "release" => {
                if session.input_owner.as_deref() != Some(actor) {
                    return Err("only the input owner can release control".into());
                }
                session.input_owner = None;
                session.queue.bytes.clear();
                session.queue.offset = 0;
                if let Some(ui) = &mut session.ui {
                    ui.cancel_input();
                }
            }
            _ => return Err("control action must be acquire or release".into()),
        }
        if let Some(ui) = &mut session.ui {
            ui.force = true;
        }
        Ok(session.info())
    }

    fn handle(&mut self, request: &Value) -> Result<Reply, String> {
        let op = request
            .get("op")
            .and_then(Value::as_str)
            .ok_or("missing operation")?;
        if op == "create" {
            return self.create(request).map(Reply::Value);
        }
        if op == "restore-layout" {
            if actor(request)? != "local" {
                return Err("restoring desktop layouts requires local control".into());
            }
            return self
                .restore_layout(request.get("layout").ok_or("layout snapshot is required")?)
                .map(Reply::Value);
        }
        if op == "list" {
            let filter = request.get("filter").and_then(Value::as_str).unwrap_or("");
            if filter.len() > 256 {
                return Err("session filter exceeds 256 bytes".into());
            }
            return Ok(Reply::Value(json::object([
                (
                    "sessions",
                    Value::Array(
                        self.sessions
                            .iter()
                            .filter(|s| s.name.contains(filter) || s.uri.contains(filter))
                            .map(Session::info)
                            .collect(),
                    ),
                ),
                ("broker_pid", Value::from(unsafe { os::getpid() })),
            ])));
        }
        let attachment = if op == "wait" {
            Some(
                request
                    .get("attachment")
                    .and_then(Value::as_u64)
                    .ok_or("attachment token is required")?,
            )
        } else {
            None
        };
        if let Some(attachment) = attachment {
            let token = format!("attachment:{attachment}");
            if let Some((_, status)) = self.completed.iter().rev().find(|(key, _)| key == &token) {
                return Ok(Reply::Value(json::object([(
                    "exit_code",
                    Value::from(*status),
                )])));
            }
        }
        let index = self.selector(request)?;
        if matches!(op, "write" | "resize" | "close" | "rename")
            && self.sessions[index].input_owner.as_deref() != Some(actor(request)?)
        {
            return Err(
                "input control is required; acquire or explicitly take over this session".into(),
            );
        }
        match op {
            "save-layout" => self.save_layout(index).map(Reply::Value),
            "info" => Ok(Reply::Value(self.sessions[index].info())),
            "commands" => {
                let commands = self.sessions[index]
                    .terminal
                    .commands()
                    .map(|command| {
                        json::object([
                            ("id", Value::from(command.id)),
                            (
                                "exit_status",
                                command.exit_status.map(Value::from).unwrap_or(Value::Null),
                            ),
                            ("available", Value::Bool(command.available)),
                            ("has_output", Value::Bool(command.output.is_some())),
                            ("complete", Value::Bool(command.end.is_some())),
                        ])
                    })
                    .collect();
                Ok(Reply::Value(json::object([(
                    "commands",
                    Value::Array(commands),
                )])))
            }
            "command-read" => {
                let id = request
                    .get("command_id")
                    .and_then(Value::as_u64)
                    .ok_or("command_id is required")?;
                let max = bounded(request, "max_bytes", 65536, 1, 65536)?;
                let part = request
                    .get("part")
                    .and_then(Value::as_str)
                    .unwrap_or("output");
                let terminal = &self.sessions[index].terminal;
                let output = match part {
                    "input" => terminal.command_input(id, max),
                    "output" => terminal.command_output(id, max),
                    _ => return Err("part must be input or output".into()),
                }
                .ok_or("command is not retained or does not contain the requested region")?;
                Ok(Reply::Value(json::object([
                    ("text", Value::from(output.text)),
                    ("truncated", Value::Bool(output.truncated)),
                    ("complete", Value::Bool(output.complete)),
                    ("command_id", Value::from(id)),
                ])))
            }
            "control" => self.control(index, request).map(Reply::Value),
            "detach" => {
                if actor(request)? != "local" {
                    return Err("detaching desktop views requires local control".into());
                }
                Ok(Reply::Value(self.detach(index)))
            }
            "rename" => {
                let name = request
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|s| valid_name(s))
                    .ok_or(
                        "session names use 1..64 ASCII letters, digits, dot, underscore or hyphen",
                    )?;
                if self.sessions.iter().enumerate().any(|(i, s)| {
                    i != index && s.origin == self.sessions[index].origin && s.name == name
                }) {
                    return Err("session name already exists in this namespace".into());
                }
                self.sessions[index].name = name.into();
                // Creation URIs remain valid after rename. Numeric IDs are also
                // accepted by the broker for the lifetime of this process.
                Ok(Reply::Value(self.sessions[index].info()))
            }
            "action" => {
                if actor(request)? != "local" {
                    return Err("desktop actions require local control".into());
                }
                let action = request
                    .get("action")
                    .and_then(Value::as_str)
                    .ok_or("action is required")?;
                self.action(index, action).map(Reply::Value)
            }
            "move" => {
                if actor(request)? != "local" {
                    return Err("moving desktop views requires local control".into());
                }
                let target = request
                    .get("target")
                    .and_then(Value::as_str)
                    .ok_or("target is required")?;
                let target_index =
                    self.selector(&json::object([("session", Value::from(target))]))?;
                self.group(
                    self.sessions[target_index].id,
                    self.sessions[index].id,
                    request
                        .get("layout")
                        .and_then(Value::as_str)
                        .unwrap_or("tab"),
                )?;
                Ok(Reply::Value(self.sessions[index].info()))
            }
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
                    .resize(pty_size(
                        cols,
                        rows,
                        session.ui.as_ref().map(|ui| &ui.window),
                    ))
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
                let read_only = request
                    .get("observe")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if let Some(ui) = &mut self.sessions[index].ui {
                    if read_only {
                        ui.read_only = true;
                        ui.owns_child = false;
                        ui.cancel_input();
                    }
                    let observing =
                        read_only || self.sessions[index].input_owner.as_deref() != Some("local");
                    if let Some(workspace) = self.workspace(self.sessions[index].id)
                        && !observing
                    {
                        self.workspaces[workspace]
                            .layout
                            .focus(self.sessions[index].id);
                        self.arrange(workspace, true)?;
                    } else if !observing {
                        self.sessions[index].ui.as_ref().unwrap().window.raise();
                    }
                    return Ok(Reply::Value(self.sessions[index].info()));
                }
                let env = environment(request)?;
                let connection = self.connection(&env)?;
                let attachment = self.next_attachment;
                self.next_attachment += 1;
                let session = &mut self.sessions[index];
                if let Some(value) = request.get("_appearance") {
                    session.appearance = Appearance::from_json(value).map_err(String::from)?;
                }
                let observing = read_only || session.input_owner.as_deref() != Some("local");
                let window = Window::new_view(
                    session.terminal.cols(),
                    session.terminal.rows(),
                    &session.appearance,
                    &connection,
                    !observing,
                )
                .map_err(String::from)?;
                if !observing {
                    session
                        .pty
                        .resize(pty_size(
                            session.terminal.cols(),
                            session.terminal.rows(),
                            Some(&window),
                        ))
                        .map_err(|e| format!("cannot update attached PTY size: {e}"))?;
                }
                session
                    .terminal
                    .set_cell_size(window.cw as u32, window.ch as u32);
                session.ui = Some(Ui::new(window, attachment));
                session.ui.as_mut().unwrap().read_only = read_only;
                Ok(Reply::Value(session.info()))
            }
            "wait" => {
                let session = &self.sessions[index];
                let attachment = attachment.ok_or("attachment token is required")?;
                if session
                    .ui
                    .as_ref()
                    .is_none_or(|ui| ui.attachment != attachment)
                {
                    return Err("window attachment has closed or been replaced".into());
                }
                Ok(Reply::Wait(format!("attachment:{attachment}")))
            }
            "close" => Ok(Reply::Value(self.retire(index))),
            _ => Err(format!("unknown session operation: {op}")),
        }
    }

    fn events(&mut self, openers: &mut os::Openers, buffers: &mut InputBuffers) {
        for connection_index in 0..self.connections.len() {
            let connection = self.connections[connection_index].1.clone();
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
                let kind = unsafe { event.kind };
                if matches!(kind, x11::FOCUS_IN | x11::FOCUS_OUT)
                    && (kind == x11::FOCUS_IN) != (x11::input_focus(connection.display) == id)
                {
                    continue;
                }
                if let Some(index) = self
                    .workspaces
                    .iter()
                    .position(|w| w.window.id == id && w.window.display() == connection.display)
                {
                    match unsafe { event.kind } {
                        x11::CONFIGURE_NOTIFY => {
                            let e = unsafe { event.configure };
                            let w = &mut self.workspaces[index].window;
                            if e.serial < w.configure_serial {
                                continue;
                            }
                            if w.width != e.width || w.height != e.height {
                                w.width = e.width.max(1);
                                w.height = e.height.max(1);
                                let _ = self.arrange(index, false);
                            }
                        }
                        x11::FOCUS_IN => {
                            let active = self.workspaces[index].layout.focused;
                            if let Some(ui) = self
                                .sessions
                                .iter()
                                .find(|s| s.id == active)
                                .and_then(|s| s.ui.as_ref())
                            {
                                ui.window.raise();
                            }
                        }
                        x11::CLIENT_MESSAGE | x11::DESTROY_NOTIFY => {
                            let destroyed = unsafe { event.kind } == x11::DESTROY_NOTIFY;
                            if destroyed
                                || unsafe { event.client.data.longs[0] as u64 }
                                    == self.workspaces[index].window.wm_delete
                            {
                                if destroyed {
                                    self.workspaces[index].window.id = 0;
                                }
                                for session in &mut self.sessions {
                                    if let Some(ui) = &mut session.ui
                                        && ui.workspace == Some(id)
                                    {
                                        ui.open = false;
                                        if destroyed {
                                            ui.window.id = 0;
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                if let Some(index) = self.sessions.iter().position(|s| {
                    s.ui.as_ref().is_some_and(|ui| {
                        ui.open
                            && ui.window.id != 0
                            && ui.window.id == id
                            && ui.window.display == connection.display
                    })
                }) {
                    let kind = unsafe { event.kind };
                    if kind == x11::KEY_RELEASE {
                        let code = (unsafe { event.key.keycode } as usize).min(255);
                        if self.shortcuts[code] {
                            self.shortcuts[code] = false;
                            continue;
                        }
                    }
                    if kind == x11::KEY_PRESS {
                        let mut e = unsafe { event.key };
                        let symbol = self.sessions[index]
                            .ui
                            .as_ref()
                            .unwrap()
                            .window
                            .key_symbol(&mut e, false);
                        if let Some(action) = actions::key_action(
                            &self.sessions[index].appearance.bindings,
                            symbol,
                            e.state,
                        ) {
                            self.shortcuts[(e.keycode as usize).min(255)] = true;
                            if let Err(error) = self.action(index, action) {
                                os::print(2, format!("ztty: {error}\n").as_bytes());
                                if let Some(ui) = &mut self.sessions[index].ui {
                                    ui.error(error);
                                }
                            }
                            continue;
                        }
                    }
                    if kind == x11::BUTTON_RELEASE {
                        let button = unsafe { event.button.button } as usize;
                        if !(1..=11).contains(&button) {
                            continue;
                        }
                        if self.mouse_shortcuts[button] {
                            self.mouse_shortcuts[button] = false;
                            continue;
                        }
                    }
                    if kind == x11::BUTTON_PRESS {
                        let e = unsafe { event.button };
                        if !(1..=11).contains(&e.button) {
                            continue;
                        }
                        if let Some(action) = actions::lookup(
                            &self.sessions[index].appearance.bindings,
                            0x1000_0000 + u64::from(e.button),
                            e.state,
                        ) {
                            self.mouse_shortcuts[e.button as usize] = true;
                            if let Err(error) = self.action(index, action) {
                                os::print(2, format!("ztty: {error}\n").as_bytes());
                                if let Some(ui) = &mut self.sessions[index].ui {
                                    ui.error(error);
                                }
                            }
                            continue;
                        }
                    }
                    if kind == x11::FOCUS_IN && !self.sessions[index].ui.as_ref().unwrap().visible {
                        continue;
                    }
                    if kind == x11::BUTTON_PRESS || kind == x11::FOCUS_IN {
                        let session_id = self.sessions[index].id;
                        if let Some(workspace) = self.workspace(session_id) {
                            self.workspaces[workspace].layout.focus(session_id);
                            if kind == x11::BUTTON_PRESS {
                                self.sessions[index].ui.as_ref().unwrap().window.raise();
                            }
                        }
                    }
                    let session = &mut self.sessions[index];
                    let ui = session.ui.as_mut().unwrap();
                    let allow_input =
                        !ui.read_only && session.input_owner.as_deref() == Some("local");
                    if ui
                        .event(
                            &mut event,
                            &mut session.terminal,
                            &session.pty,
                            &mut session.queue,
                            openers,
                            buffers,
                            allow_input,
                        )
                        .is_err()
                    {
                        ui.open = false;
                    }
                    if let Some(action) = ui.pending_action.take()
                        && let Err(error) = self.action(index, action)
                    {
                        os::print(2, format!("ztty: {error}\n").as_bytes());
                        if let Some(ui) = &mut self.sessions[index].ui {
                            ui.error(error);
                        }
                    }
                }
            }
        }
    }

    fn tick(&mut self, buffer: &mut [u8]) -> Option<u64> {
        let changed = self.watcher.changed();
        if !changed.is_empty() {
            for index in 0..self.sessions.len() {
                if self.sessions[index]
                    .config_path
                    .as_ref()
                    .is_some_and(|p| changed.contains(p))
                    && let Err(error) = self.reload(index)
                {
                    os::print(2, format!("ztty: config reload: {error}\n").as_bytes());
                    if let Some(ui) = &mut self.sessions[index].ui {
                        ui.error(error);
                    }
                }
            }
        }
        let mut deadline = None;
        for session in &mut self.sessions {
            session.pump(buffer);
            if let Some(ui) = &mut session.ui
                && ui.open
                && ui.visible
                && ui.window.id != 0
                && let Some(next) = ui.paint(
                    &mut session.terminal,
                    &session.appearance,
                    ui.read_only || session.input_owner.as_deref() != Some("local"),
                )
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
            let observing = self.sessions[i].input_owner.as_deref() != Some("local")
                || self.sessions[i].ui.as_ref().is_some_and(|ui| ui.read_only);
            let owns_child = self.sessions[i].input_owner.as_deref() == Some("local")
                && self.sessions[i].ui.as_ref().is_some_and(|ui| ui.owns_child);
            if closed && observing && !ended && !owns_child {
                // An attached observer cannot terminate another actor's live PTY.
                self.detach(i);
            } else if !self.sessions[i].persistent && (closed || ended) {
                self.retire(i);
            } else if closed {
                self.detach(i);
            }
        }
        for workspace in &mut self.workspaces {
            if let Some(session) = self
                .sessions
                .iter()
                .find(|s| s.id == workspace.layout.focused)
            {
                let label = if session.terminal.title().is_empty() {
                    &session.name
                } else {
                    session.terminal.title()
                };
                let observer = session.input_owner.as_deref() != Some("local")
                    || session.ui.as_ref().is_some_and(|ui| ui.read_only);
                let title = format!(
                    "ztty · {}/{} · {}{}",
                    workspace.layout.active_tab + 1,
                    workspace.layout.tabs.len(),
                    label,
                    if observer { " · observe" } else { "" }
                );
                if title != workspace.title {
                    workspace.window.title(&title);
                    workspace.title = title;
                }
            }
        }
        // Keep each display connection for the broker lifetime. A persistent PTY
        // may reopen a view after its last window closes; reusing X11/XIM state
        // also avoids repeatedly initializing display extensions. Font entries
        // are weak and still release when their last window is dropped.
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
    let mut buffers = InputBuffers {
        text: alloc::vec![0u8;4096],
        encoded: Vec::with_capacity(128),
    };
    let mut fds = Vec::new();
    let mut idle_since = os::milliseconds();
    loop {
        pool.events(&mut openers, &mut buffers);
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
        if pool.watcher.fd() >= 0 {
            fds.push(os::PollFd {
                fd: pool.watcher.fd(),
                events: os::POLLIN,
                revents: 0,
            });
        }
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
