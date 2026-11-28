//! Shared PTYs and desktop windows, driven by one poll loop.
use crate::{
    Appearance, actions,
    backend::{self, Backend, Preference, Requirements},
    broker, config_watch,
    display::{Connection, Window},
    json::{self, Value},
    layout::Layout,
    os, plugin_host,
    repaint::Repaint,
    search,
    session_core::{ReadStep, SessionCore},
    x11::{self, Selection},
};
use alloc::{
    boxed::Box, collections::VecDeque, ffi::CString, format, rc::Rc, string::String, vec::Vec,
};
use core::ffi::c_int;
use zterm::CursorShape;

struct InputBuffers {
    text: Vec<u8>,
    encoded: Vec<u8>,
}

struct Search {
    cache: Box<search::Cache>,
    query: String,
    case: search::Case,
    hit: Option<search::Hit>,
    count: usize,
    ordinal: usize,
}

struct ClipboardMenu {
    bounds: crate::layout::Rect,
    selected: usize,
    enabled: [bool; 2],
}

struct Palette {
    query: String,
    selected: usize,
    sessions: bool,
    items: Vec<PaletteItem>,
}

#[derive(Clone, Copy)]
enum PaletteTarget {
    Action(&'static str),
    Plugin(usize),
    CancelPlugin,
    Session {
        id: u64,
        attach: bool,
        observe: bool,
    },
}

struct PaletteItem {
    target: PaletteTarget,
    label: String,
    detail: String,
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

mod osc52;
mod queries;
mod reload;
mod requests;
pub(crate) use reload::{reload_fds, serve_reloaded, validate_reload};
mod view;
mod view_input;
mod workspaces;

struct Ui {
    window: Window,
    workspace: Option<u64>,
    visible: bool,
    read_only: bool,
    // A newly launched window owns its child; attaching only owns a view.
    owns_child: bool,
    search: Option<Search>,
    palette: Option<Palette>,
    clipboard_menu: Option<ClipboardMenu>,
    osc_clipboard: Option<osc52::Pending>,
    pending_action: Option<PaletteTarget>,
    plugin_running: bool,
    message: Option<(String, u64)>,
    command: Option<u64>,
    attachment: u64,
    selection: Option<Selection>,
    selection_unit: ztty::selection::Unit,
    selection_anchor: ((usize, usize), (usize, usize)),
    clicks: Option<(u64, (usize, usize), u8)>,
    history: usize,
    new_output: bool,
    scroll_prompt: Option<crate::layout::Rect>,
    native_ui: ztty::native_ui::View,
    force: bool,
    repaint: Repaint,
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
    text_blink: ztty::text_blink::TextBlink,
}

struct Session {
    ssh_profile: Option<Value>,
    id: u64,
    created_at: u64,
    uri: String,
    name: String,
    origin: String,
    appearance: Appearance,
    core: SessionCore,
    ui: Option<Ui>,
    last_view_backend: Option<Backend>,
    persistent: bool,
    launch_env: Vec<CString>,
    initial_cwd: CString,
    config_path: Option<CString>,
    base_font: String,
}

impl Session {
    fn status(&self) -> Option<i32> {
        self.core
            .pty
            .status()
            .map(|s| s.code().unwrap_or(128 + s.signal().unwrap_or(0)))
    }
    fn pump(&mut self, buffer: &mut [u8]) {
        for _ in 0..4 {
            match self.core.read_once(buffer, self.persistent) {
                Ok(ReadStep::Data {
                    history_scrolled,
                    history_after,
                    ..
                }) => {
                    if let Some(ui) = &mut self.ui {
                        // Input priority survives a force/expose paint of the old
                        // screen and is consumed only after subsequent PTY output.
                        ui.repaint.output_received();
                        if ui.history != 0 {
                            ui.new_output = true;
                            ui.set_history(
                                ui.history
                                    .saturating_add(history_scrolled)
                                    .min(history_after),
                            );
                            ui.force = true;
                        }
                    }
                }
                Ok(ReadStep::Pending | ReadStep::Eof) | Err(_) => break,
            }
        }
        if !self.core.queue_responses() {
            os::print(2, b"ztty: input queue full; paste was rejected\n");
        }
        let _ = self.core.flush_input();
        let _ = self.core.pty.reap();
    }
}
struct Workspace {
    window: x11::Container,
    layout: Layout,
    title: String,
    reveal_pending: bool,
    focus_after_reveal: bool,
}

#[derive(Clone, Copy)]
struct NativeGesture {
    session: u64,
    attachment: u64,
    display: *mut x11::Display,
    button: u32,
}

struct Pool {
    pending_reload: Option<CString>,
    sessions: Vec<Session>,
    workspaces: Vec<Workspace>,
    connections: Vec<(String, Rc<Connection>)>,
    next: u64,
    next_attachment: u64,
    completed: VecDeque<(String, i32)>,
    finished: Vec<(String, i32)>,
    reaping: Vec<(i32, u64)>,
    plugins: Vec<PluginRun>,
    shortcuts: [bool; 256],
    mouse_shortcuts: [bool; 12],
    native_gestures: Vec<NativeGesture>,
    watcher: config_watch::Watcher,
}

struct ViewSelection {
    preference: Preference,
    previous: Option<Backend>,
    workspace: bool,
}

// Jobs outlive a disappearing view until their direct worker has been reaped.
// Session and attachment identities prevent an old result reaching a new view.
struct PluginRun {
    session: u64,
    attachment: u64,
    window: x11::Xid,
    registration: ztty::text_plugins::Registration,
    job: plugin_host::Job,
}

impl Pool {
    fn new() -> Self {
        Self {
            pending_reload: None,
            sessions: Vec::new(),
            workspaces: Vec::new(),
            connections: Vec::new(),
            next: 1,
            next_attachment: 1,
            completed: VecDeque::new(),
            finished: Vec::new(),
            reaping: Vec::new(),
            plugins: Vec::new(),
            shortcuts: [false; 256],
            mouse_shortcuts: [false; 12],
            native_gestures: Vec::new(),
            watcher: config_watch::Watcher::new(),
        }
    }

    fn connection(&mut self, env: &[CString], wayland: bool) -> Result<Rc<Connection>, String> {
        let value = |prefix: &[u8]| env.iter().find_map(|e| e.to_bytes().strip_prefix(prefix));
        if wayland {
            let key = format!(
                "wayland:{}:{}",
                String::from_utf8_lossy(value(b"XDG_RUNTIME_DIR=").unwrap_or_default()),
                String::from_utf8_lossy(value(b"WAYLAND_DISPLAY=").unwrap_or(b"wayland-0"))
            );
            if let Some((_, c)) = self.connections.iter().find(|(name, _)| *name == key) {
                return Ok(c.clone());
            }
            let c = Connection::wayland(env).map_err(String::from)?;
            self.connections.push((key, c.clone()));
            return Ok(c);
        }
        let display = value(b"DISPLAY=");
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

    fn open_window(
        &mut self,
        env: &[CString],
        selection: ViewSelection,
        size: (usize, usize),
        appearance: &Appearance,
        activate: bool,
    ) -> Result<Window, String> {
        // A native view currently prevents executable handover for the whole
        // pool. Auto must not remove reload from an existing X11/headless pool.
        let native_compatible = self.sessions.is_empty()
            || self.sessions.iter().any(|session| {
                session
                    .ui
                    .as_ref()
                    .is_some_and(|ui| ui.window.backend() == "wayland")
            });
        backend::initialize(
            selection.preference.for_attachment(selection.previous),
            Requirements {
                workspace: selection.workspace,
                native_compatible,
                native_compiled: cfg!(feature = "wayland"),
            },
            |backend| {
                // XOpenDisplay(NULL) reads the broker's process environment.
                // Auto must use the requesting client's display, not borrow a
                // stale DISPLAY from the process that first started the pool.
                if selection.preference == Preference::Auto
                    && backend == Backend::X11
                    && env
                        .iter()
                        .find_map(|entry| entry.to_bytes().strip_prefix(b"DISPLAY="))
                        .is_none_or(|value| value.is_empty())
                {
                    return Err("DISPLAY is not set in the requesting client".into());
                }
                let connection = self.connection(env, backend == Backend::Wayland)?;
                // Include renderer/font/native-surface setup in this attempt;
                // obtaining a logical connection alone does not prove that a
                // usable window exists. No PTY operation belongs in this closure.
                if activate {
                    Window::new_shared(size.0, size.1, appearance, &connection)
                } else {
                    Window::new_view(size.0, size.1, appearance, &connection, false)
                }
                .map_err(String::from)
            },
        )
    }

    fn open_palette(&mut self, index: usize, sessions: bool) -> Result<(), String> {
        let source = &self.sessions[index];
        let source_ui = source.ui.as_ref().ok_or("session has no window")?;
        let source_observing = source_ui.read_only || !source.core.owns("local");
        let mut items: Vec<PaletteItem> = if sessions {
            self.sessions
                .iter()
                .filter(|session| {
                    session
                        .ui
                        .as_ref()
                        .is_none_or(|ui| ui.open && ui.window.id != 0)
                })
                .map(|session| {
                    let attach = session.ui.is_none();
                    let observe = session
                        .ui
                        .as_ref()
                        .map_or(source_observing || !session.core.owns("local"), |ui| {
                            ui.read_only || !session.core.owns("local")
                        });
                    let scope = if attach {
                        "Attach"
                    } else if session.id == source.id {
                        "Current"
                    } else if source_ui.workspace.is_some()
                        && session
                            .ui
                            .as_ref()
                            .is_some_and(|ui| ui.workspace == source_ui.workspace)
                    {
                        "Tab/pane"
                    } else {
                        "Window"
                    };
                    let title: String = session
                        .core
                        .terminal
                        .title()
                        .chars()
                        .filter(|ch| !ch.is_control())
                        .take(80)
                        .collect();
                    let label = format!(
                        "[{scope}{}] {}{}{}",
                        if observe { " · read-only" } else { "" },
                        session.name,
                        if title.is_empty() { "" } else { " — " },
                        title,
                    );
                    PaletteItem {
                        target: PaletteTarget::Session {
                            id: session.id,
                            attach,
                            observe,
                        },
                        label,
                        detail: session.uri.clone(),
                    }
                })
                .collect()
        } else {
            actions::matching("")
                .map(|action| PaletteItem {
                    target: PaletteTarget::Action(action.name),
                    label: action.label.into(),
                    detail: actions::shortcut(&source.appearance.bindings, action.name)
                        .unwrap_or_default(),
                })
                .collect()
        };
        if !sessions {
            for (index, plugin) in source.appearance.text_plugins.iter().enumerate() {
                items.push(PaletteItem {
                    target: PaletteTarget::Plugin(index),
                    label: format!("Copy using {}", plugin.name),
                    detail: "Plugin · selection or visible text".into(),
                });
            }
            if source_ui.plugin_running {
                items.push(PaletteItem {
                    target: PaletteTarget::CancelPlugin,
                    label: "Cancel running plugin".into(),
                    detail: "Esc".into(),
                });
            }
        }
        let ui = self.sessions[index].ui.as_mut().unwrap();
        ui.search = None;
        ui.clipboard_menu = None;
        ui.window.search_matches(0, Vec::new());
        ui.palette = Some(Palette {
            query: String::new(),
            selected: 0,
            sessions,
            items,
        });
        ui.force = true;
        Ok(())
    }

    fn attach_view(
        &mut self,
        index: usize,
        read_only: bool,
        connection: &Rc<Connection>,
        appearance: Option<&Value>,
    ) -> Result<(), String> {
        let session = &self.sessions[index];
        let appearance = appearance
            .map(Appearance::from_json)
            .transpose()
            .map_err(String::from)?
            .unwrap_or_else(|| session.appearance.clone());
        let observing = read_only || !session.core.owns("local");
        let window = Window::new_view(
            session.core.terminal.cols(),
            session.core.terminal.rows(),
            &appearance,
            connection,
            !observing,
        )
        .map_err(String::from)?;
        self.commit_view(index, read_only, window, appearance)
    }

    fn commit_view(
        &mut self,
        index: usize,
        read_only: bool,
        window: Window,
        appearance: Appearance,
    ) -> Result<(), String> {
        let backend = Backend::parse(window.backend()).map_err(String::from)?;
        let attachment = self.next_attachment;
        self.next_attachment += 1;
        let session = &mut self.sessions[index];
        let observing = read_only || !session.core.owns("local");
        if !observing {
            session
                .core
                .resize(pty_size(
                    session.core.terminal.cols(),
                    session.core.terminal.rows(),
                    Some(&window),
                ))
                .map_err(|e| format!("cannot update attached PTY size: {e}"))?;
        }
        session
            .core
            .terminal
            .set_cell_size(window.cw as u32, window.ch as u32);
        let mut ui = Ui::new(window, attachment);
        ui.read_only = read_only;
        session.appearance = appearance;
        session.last_view_backend = Some(backend);
        session.ui = Some(ui);
        session.core.terminal.set_ui_host_enabled(true);
        Ok(())
    }

    fn select_session(
        &mut self,
        source: usize,
        id: u64,
        attach: bool,
        observe: bool,
    ) -> Result<Value, String> {
        let target = self
            .sessions
            .iter()
            .position(|session| session.id == id)
            .ok_or("session has closed; reopen the session switcher")?;
        if self.sessions[target]
            .ui
            .as_ref()
            .is_some_and(|ui| !ui.open || ui.window.id == 0)
        {
            return Err("session view is closing; reopen the session switcher".into());
        }
        if self.sessions[target].ui.is_none() {
            if !attach {
                return Err("session detached; reopen the switcher to attach it".into());
            }
            let source = &self.sessions[source];
            let ui = source.ui.as_ref().ok_or("source view has closed")?;
            let read_only = observe
                || ui.read_only
                || !source.core.owns("local")
                || !self.sessions[target].core.owns("local");
            let connection = self
                .connections
                .iter()
                .find(|(_, connection)| connection.display == ui.window.display)
                .map(|(_, connection)| connection.clone())
                .ok_or("source display is unavailable")?;
            self.attach_view(target, read_only, &connection, None)?;
        }
        // Focus an existing view in its own workspace. This never reparents a
        // window, replaces a PTY, changes its observer flag or acquires input.
        if let Some(workspace) = self.workspace(id) {
            self.workspaces[workspace].layout.focus(id);
            self.arrange(workspace, true)?;
        } else {
            let ui = self.sessions[target].ui.as_mut().unwrap();
            ui.visible = true;
            ui.force = true;
            ui.window.raise();
        }
        Ok(self.sessions[target].info())
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
                    ui.start_search(&session.core.terminal);
                }
            }
            "palette" | "sessions" => self.open_palette(index, action == "sessions")?,
            "none" => {}
            "copy" => {
                let session = &mut self.sessions[index];
                if let Some(ui) = &mut session.ui {
                    ui.clipboard_menu = None;
                    let text = if ui.search.is_some() {
                        let text = ui
                            .search
                            .as_ref()
                            .and_then(|search| search.hit)
                            .and_then(|hit| search::text(&session.core.terminal, hit));
                        if text.is_some() {
                            text
                        } else {
                            ui.find(&session.core.terminal, false, true);
                            ui.search
                                .as_ref()
                                .and_then(|search| search.hit)
                                .and_then(|hit| search::text(&session.core.terminal, hit))
                        }
                    } else {
                        ui.selection
                            .map(|selection| selection.text(&session.core.terminal, ui.history))
                    };
                    if let Some(text) = text.filter(|text| !text.is_empty()) {
                        ui.window.copy(text, false);
                        ui.notice("Copied to clipboard");
                    } else {
                        ui.notice(
                            "Select text first · Drag, double-click a word, or triple-click a line",
                        );
                    }
                }
            }
            "paste" | "paste-primary" => {
                let session = &mut self.sessions[index];
                if let Some(ui) = &mut session.ui {
                    ui.clipboard_menu = None;
                    ui.force = true;
                }
                if !session.core.owns("local") || session.ui.as_ref().is_some_and(|ui| ui.read_only)
                {
                    return Err("observing views cannot paste; take input control first".into());
                }
                if let Some(ui) = &mut session.ui {
                    ui.manual_paste(action == "paste-primary");
                }
            }
            "scroll-up" | "scroll-down" => {
                let session = &mut self.sessions[index];
                if let Some(ui) = &mut session.ui {
                    let step = session.core.terminal.rows().saturating_sub(1).max(1);
                    ui.set_history(if action == "scroll-up" {
                        ui.history
                            .saturating_add(step)
                            .min(session.core.terminal.history_len())
                    } else {
                        ui.history.saturating_sub(step)
                    });
                    ui.selection = None;
                    ui.force = true;
                }
            }
            "previous-command" | "next-command" => {
                let session = &mut self.sessions[index];
                let ui = session.ui.as_mut().ok_or("session has no window")?;
                let command = if action == "previous-command" {
                    session.core.terminal.commands().rev().find(|c| {
                        ui.command.is_none_or(|id| c.id < id)
                            && session.core.terminal.command_scroll_offset(c.id).is_some()
                    })
                } else {
                    session.core.terminal.commands().find(|c| {
                        ui.command.is_some_and(|id| c.id > id)
                            && session.core.terminal.command_scroll_offset(c.id).is_some()
                    })
                };
                let command = command
                    .ok_or("no shell command in that direction; enable shell integration")?;
                let offset = session
                    .core
                    .terminal
                    .command_scroll_offset(command.id)
                    .ok_or("command has left the retained history")?;
                ui.command = Some(command.id);
                ui.set_history(offset);
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
                            .core
                            .terminal
                            .commands()
                            .rev()
                            .find(|c| c.output.is_some())
                            .map(|c| c.id)
                    })
                    .ok_or("no command output; enable shell integration")?;
                let output = session
                    .core
                    .terminal
                    .command_output(id, 65536)
                    .ok_or("command output is unavailable")?;
                if output.truncated {
                    return Err("command output is truncated; use the bounded command-output API to inspect it explicitly".into());
                }
                ui.window.copy(output.text, false);
                ui.clipboard_menu = None;
                ui.notice("Copied command output to clipboard");
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

    fn start_plugin(&mut self, index: usize, plugin: usize) -> Result<Value, String> {
        if self.plugins.len() >= ztty::text_plugins::MAX_PLUGINS {
            return Err("too many running extensions; wait for one to finish".into());
        }
        let session = &mut self.sessions[index];
        let ui = session.ui.as_mut().ok_or("session has no window")?;
        if ui.plugin_running {
            return Err("an extension is already running; Esc cancels it".into());
        }
        let registration = session
            .appearance
            .text_plugins
            .get(plugin)
            .ok_or("extension is no longer registered")?
            .clone();
        let terminal = &session.core.terminal;
        let text = ui
            .selection
            .unwrap_or(Selection {
                start: (0, 0),
                end: (terminal.cols() - 1, terminal.rows() - 1),
                dragging: false,
            })
            .text(terminal, ui.history);
        if text.is_empty() {
            return Err("no text to send to the extension".into());
        }
        let job = plugin_host::Job::start(&registration.path, plugin_host::Mode::Run, &text)?;
        ui.plugin_running = true;
        ui.message = Some((
            format!("Running {} · Esc cancels", registration.name),
            os::milliseconds() + 2500,
        ));
        ui.force = true;
        self.plugins.push(PluginRun {
            session: session.id,
            attachment: ui.attachment,
            window: ui.window.id,
            registration,
            job,
        });
        Ok(session.info())
    }

    fn cancel_plugin(&mut self, index: usize) -> Result<Value, String> {
        let session = &mut self.sessions[index];
        if let Some(ui) = &mut session.ui {
            for run in &mut self.plugins {
                if run.session == session.id && run.attachment == ui.attachment {
                    run.job.cancel("extension cancelled");
                }
            }
            ui.notice("Cancelling extension");
        }
        Ok(session.info())
    }

    fn poll_plugins(&mut self) -> Option<u64> {
        if self.plugins.is_empty() {
            return None;
        }
        let now = os::milliseconds();
        let mut deadline: Option<u64> = None;
        for index in (0..self.plugins.len()).rev() {
            let run = &mut self.plugins[index];
            let owner = self.sessions.iter_mut().find(|session| {
                session.id == run.session
                    && session.ui.as_ref().is_some_and(|ui| {
                        ui.attachment == run.attachment && ui.window.id == run.window
                    })
            });
            let active = owner.as_ref().is_some_and(|session| {
                session
                    .ui
                    .as_ref()
                    .is_some_and(|ui| ui.open && ui.visible && ui.window.id != 0)
                    && session.appearance.text_plugins.contains(&run.registration)
            });
            if !active {
                run.job
                    .cancel("extension cancelled: its view or registration changed");
            }
            if let Some(result) = run.job.step(now) {
                if let Some(session) = owner {
                    let ui = session.ui.as_mut().unwrap();
                    ui.plugin_running = false;
                    if active {
                        match result {
                            Ok(text) if !text.is_empty() => {
                                ui.window.copy(text, false);
                                ui.notice(&format!("Copied using {}", run.registration.name));
                            }
                            Ok(_) => ui.notice("Extension returned no text; clipboard unchanged"),
                            Err(error) => ui.error(error),
                        }
                    }
                }
                self.plugins.swap_remove(index);
            } else {
                let next = run.job.next_deadline(now);
                deadline = Some(deadline.map_or(next, |old| old.min(next)));
            }
        }
        deadline
    }

    fn apply_appearance(
        &mut self,
        index: usize,
        appearance: Appearance,
        reset_font: bool,
    ) -> Result<Value, String> {
        let session = &mut self.sessions[index];
        if let Some(ui) = &mut session.ui {
            if session.appearance.text_plugins != appearance.text_plugins {
                // A palette index belongs to its current registry snapshot.
                ui.palette = None;
            }
            let allow_resize = session.core.owns("local") && !ui.read_only;
            let mut anchor = ui.capture_view_anchor(&session.core.terminal);
            let (cols, rows) = ui.window.reconfigure(&appearance, |cols, rows, cw, ch| {
                if allow_resize && !session.core.eof() {
                    session
                        .core
                        .resize_with_cell_size_anchored(
                            zpty::Size {
                                cols: cols as u16,
                                rows: rows as u16,
                                pixel_width: (cols * cw as usize).min(u16::MAX as usize) as u16,
                                pixel_height: (rows * ch as usize).min(u16::MAX as usize) as u16,
                            },
                            (cw as u32, ch as u32),
                            anchor.anchors_mut(),
                        )
                        .map_err(|e| format!("cannot resize reconfigured PTY: {e}"))?;
                }
                Ok(())
            })?;
            ui.native_ui.invalidate();
            session
                .core
                .terminal
                .set_cell_size(ui.window.cw as u32, ui.window.ch as u32);
            if allow_resize && session.core.eof() {
                session
                    .core
                    .terminal
                    .resize_anchored(cols, rows, anchor.anchors_mut());
            }
            ui.restore_view_anchor(&session.core.terminal, anchor);
            ui.scroll_prompt = None;
            ui.last_cursor = None;
            ui.force = true;
            ui.reset_blink = true;
            ui.message = None;
        }
        session.core.terminal.set_theme(appearance.theme);
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

    fn cancel_native_gesture(&mut self, gesture: NativeGesture) {
        if let Some(ui) = self
            .sessions
            .iter_mut()
            .find(|session| session.id == gesture.session)
            .and_then(|session| session.ui.as_mut())
            .filter(|ui| ui.attachment == gesture.attachment)
        {
            ui.native_ui.cancel_button(gesture.button);
        }
    }

    fn events(
        &mut self,
        openers: &mut os::Openers,
        buffers: &mut InputBuffers,
        read_buffer: &mut [u8],
    ) {
        for connection_index in 0..self.connections.len() {
            let connection = self.connections[connection_index].1.clone();
            for _ in 0..512 {
                let Some(mut event) = connection.next_event() else {
                    break;
                };
                let id = event.window();
                let kind = unsafe { event.kind };
                if matches!(kind, x11::BUTTON_PRESS | x11::BUTTON_RELEASE) {
                    let button = unsafe { event.button.button };
                    if let Some(position) = self.native_gestures.iter().position(|gesture| {
                        gesture.display == connection.display && gesture.button == button
                    }) {
                        let gesture = self.native_gestures.swap_remove(position);
                        let same_view = self.sessions.iter().any(|session| {
                            session.id == gesture.session
                                && session.ui.as_ref().is_some_and(|ui| {
                                    ui.open
                                        && ui.visible
                                        && ui.attachment == gesture.attachment
                                        && ui.window.id == id
                                        && ui.window.display == connection.display
                                })
                        });
                        if kind == x11::BUTTON_PRESS || !same_view {
                            // A new press supersedes a release delivered outside our
                            // windows. Cross-view releases belong to the original UI.
                            self.cancel_native_gesture(gesture);
                        }
                        if kind == x11::BUTTON_RELEASE && !same_view {
                            continue;
                        }
                    }
                }
                if kind == x11::MOTION_NOTIFY {
                    let state = unsafe { event.motion.state };
                    for position in (0..self.native_gestures.len()).rev() {
                        let gesture = self.native_gestures[position];
                        if gesture.display == connection.display
                            && gesture.button <= 5
                            && state & (1 << (gesture.button + 7)) == 0
                        {
                            // Unmapping a view drops X11's implicit grab. A
                            // release outside this client may never be delivered.
                            let gesture = self.native_gestures.swap_remove(position);
                            self.cancel_native_gesture(gesture);
                        }
                    }
                    if self
                        .native_gestures
                        .iter()
                        .any(|gesture| gesture.display == connection.display && gesture.button <= 5)
                    {
                        continue;
                    }
                }
                if matches!(kind, x11::FOCUS_IN | x11::FOCUS_OUT)
                    && (kind == x11::FOCUS_IN) != (connection.input_focus() == id)
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
                            && ui.window.accepts_event(id, unsafe { event.kind })
                            && ui.window.display == connection.display
                    })
                }) {
                    let kind = unsafe { event.kind };
                    // An outgoing tab may temporarily cover the incoming
                    // frame. Its pixels are a placeholder, not an input target.
                    if !self.sessions[index].ui.as_ref().unwrap().visible
                        && matches!(
                            kind,
                            x11::BUTTON_PRESS | x11::BUTTON_RELEASE | x11::MOTION_NOTIFY
                        )
                    {
                        // The implicit X11 pointer grab can deliver the
                        // shortcut's release to this outgoing placeholder.
                        // Retire global bookkeeping without forwarding input.
                        if kind == x11::BUTTON_RELEASE {
                            let button = unsafe { event.button.button } as usize;
                            if (1..=11).contains(&button) {
                                self.mouse_shortcuts[button] = false;
                            }
                        }
                        continue;
                    }
                    let menu_handled = self.sessions[index]
                        .ui
                        .as_mut()
                        .unwrap()
                        .clipboard_menu_pointer(&event);
                    if menu_handled {
                        let pending = self.sessions[index]
                            .ui
                            .as_mut()
                            .unwrap()
                            .pending_action
                            .take();
                        if let Some(PaletteTarget::Action(action)) = pending
                            && let Err(error) = self.action(index, action)
                            && let Some(ui) = &mut self.sessions[index].ui
                        {
                            ui.error(error);
                        }
                        continue;
                    }
                    if self.sessions[index]
                        .ui
                        .as_mut()
                        .unwrap()
                        .scroll_prompt_event(&event)
                    {
                        if kind == x11::BUTTON_PRESS {
                            let session_id = self.sessions[index].id;
                            if let Some(workspace) = self.workspace(session_id) {
                                self.workspaces[workspace].layout.focus(session_id);
                            }
                            self.sessions[index].ui.as_ref().unwrap().window.raise();
                        }
                        continue;
                    }
                    let native_handled = {
                        let session = &mut self.sessions[index];
                        let ui = session.ui.as_mut().unwrap();
                        let allow_input = !ui.read_only && session.core.owns("local");
                        ui.native_pointer(&event, &mut session.core, allow_input)
                    };
                    if native_handled {
                        if kind == x11::BUTTON_PRESS {
                            let session_id = self.sessions[index].id;
                            let attachment = self.sessions[index].ui.as_ref().unwrap().attachment;
                            self.native_gestures.push(NativeGesture {
                                session: session_id,
                                attachment,
                                display: connection.display,
                                button: unsafe { event.button.button },
                            });
                            if let Some(workspace) = self.workspace(session_id) {
                                self.workspaces[workspace].layout.focus(session_id);
                            }
                            self.sessions[index].ui.as_ref().unwrap().window.raise();
                        }
                        continue;
                    }
                    if kind == x11::KEY_RELEASE {
                        let code = (unsafe { event.key.keycode } as usize).min(255);
                        if self.shortcuts[code] {
                            self.shortcuts[code] = false;
                            continue;
                        }
                    }
                    if kind == x11::KEY_PRESS {
                        let mut e = unsafe { event.key };
                        let window = &self.sessions[index].ui.as_ref().unwrap().window;
                        let symbol = window.input_symbol(&mut e);
                        let bindings = &self.sessions[index].appearance.bindings;
                        let shifted = (e.state & 1 != 0
                            && (!bindings.is_empty() || e.state & 4 != 0))
                            .then(|| window.key_symbol(&mut e, true));
                        if let Some(action) =
                            actions::key_action_with_shifted(bindings, symbol, shifted, e.state)
                            && (self.sessions[index]
                                .ui
                                .as_ref()
                                .unwrap()
                                .native_ui
                                .focused_node()
                                .is_none()
                                || matches!(
                                    action,
                                    "font-increase" | "font-decrease" | "font-reset"
                                ))
                        {
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
                    let allow_input =
                        !session.ui.as_ref().unwrap().read_only && session.core.owns("local");
                    if allow_input && kind == x11::CONFIGURE_NOTIFY {
                        let e = unsafe { event.configure };
                        let window = &session.ui.as_ref().unwrap().window;
                        if e.serial >= window.configure_serial.get()
                            && (window.width != e.width.max(1) || window.height != e.height.max(1))
                        {
                            // Process queued output with the old grid before
                            // applying the new width, within the usual budget.
                            // A shell may already have written its first prompt.
                            session.pump(read_buffer);
                        }
                    }
                    let ui = session.ui.as_mut().unwrap();
                    if ui
                        .event(&mut event, &mut session.core, openers, buffers, allow_input)
                        .is_err()
                    {
                        ui.open = false;
                    }
                    if let Some(action) = ui.pending_action.take() {
                        let result = match action {
                            PaletteTarget::Action(action) => self.action(index, action),
                            PaletteTarget::Plugin(plugin) => self.start_plugin(index, plugin),
                            PaletteTarget::CancelPlugin => self.cancel_plugin(index),
                            PaletteTarget::Session {
                                id,
                                attach,
                                observe,
                            } => self.select_session(index, id, attach, observe),
                        };
                        if let Err(error) = result {
                            os::print(2, format!("ztty: {error}\n").as_bytes());
                            if let Some(ui) = &mut self.sessions[index].ui {
                                ui.error(error);
                            }
                        }
                    }
                }
            }
        }
    }

    fn tick(&mut self, buffer: &mut [u8], server: &mut broker::Server) -> Option<u64> {
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
        let mut deadline = self.poll_plugins();
        for session in &mut self.sessions {
            session.pump(buffer);
            let local_owner = session.core.owns("local");
            let alive = !session.core.eof();
            if let Some(ui) = &mut session.ui {
                if let Some(next) = ui.clipboard_tick(&mut session.core, local_owner && alive) {
                    deadline = Some(deadline.map_or(next, |old| old.min(next)));
                }
            } else {
                session.core.deny_clipboard_requests();
            }
            if !session
                .ui
                .as_ref()
                .is_some_and(|ui| ui.open && ui.visible && ui.window.id != 0)
            {
                session
                    .core
                    .terminal
                    .advance_graphics(os::milliseconds(), 0, false);
            }
            if let Some(ui) = &mut session.ui
                && ui.open
                && ui.visible
                && ui.window.id != 0
                && let Some(next) = ui.paint(
                    &mut session.core.terminal,
                    &session.appearance,
                    ui.read_only || !local_owner,
                    alive,
                )
            {
                deadline = Some(deadline.map_or(next, |old: u64| old.min(next)));
            }
            if session.core.eof() && session.status().is_none() {
                let next = os::milliseconds() + 20;
                deadline = Some(deadline.map_or(next, |old| old.min(next)));
            }
        }
        for i in (0..self.sessions.len()).rev() {
            let closed = self.sessions[i].ui.as_ref().is_some_and(|ui| !ui.open);
            let ended = self.sessions[i].core.eof() && self.sessions[i].status().is_some();
            let observing = !self.sessions[i].core.owns("local")
                || self.sessions[i].ui.as_ref().is_some_and(|ui| ui.read_only);
            let owns_child = self.sessions[i].core.owns("local")
                && self.sessions[i].ui.as_ref().is_some_and(|ui| ui.owns_child);
            if closed && observing && !ended && !owns_child {
                // An attached observer cannot terminate another actor's live PTY.
                self.detach(i);
            } else if !self.sessions[i].persistent && (closed || ended) {
                if ended {
                    // Headless CLI waiters already bound to this local PTY can
                    // collect its final page immediately before normal retirement.
                    let session = &self.sessions[i];
                    server.wake_reads(|wait| {
                        if wait.session_id == session.id && wait.until.is_some() {
                            session.wait_output(wait).transpose()
                        } else {
                            None
                        }
                    });
                }
                self.retire(i);
            } else if closed {
                self.detach(i);
            }
        }
        for index in 0..self.workspaces.len() {
            self.reveal_workspace(index);
        }
        for workspace in &mut self.workspaces {
            if let Some(session) = self
                .sessions
                .iter()
                .find(|s| s.id == workspace.layout.focused)
            {
                let label = if session.core.terminal.title().is_empty() {
                    &session.name
                } else {
                    session.core.terminal.title()
                };
                let observer = !session.core.owns("local")
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
    serve_loop(broker::Server::new()?, Pool::new())
}

fn serve_loop(mut server: broker::Server, mut pool: Pool) -> Result<(), String> {
    let mut openers = os::Openers::new();
    let mut read_buffer = [0u8; 16384];
    let mut buffers = InputBuffers {
        text: alloc::vec![0u8;4096],
        encoded: Vec::with_capacity(128),
    };
    let mut fds = Vec::new();
    let mut idle_since = os::milliseconds();
    loop {
        crate::gpu_worker::reap();
        pool.events(&mut openers, &mut buffers, &mut read_buffer);
        let deadline = pool.tick(&mut read_buffer, &mut server);
        server.pump(|request| pool.handle(request));
        for (uri, status) in pool.finished.drain(..) {
            server.finish(&uri, status);
        }
        server.wake_reads(
            |wait| match pool.sessions.iter().find(|s| s.id == wait.session_id) {
                Some(session) => session.wait_output(wait).transpose(),
                None => Some(Err("session closed while waiting for output".into())),
            },
        );
        // A disconnected, rejected or completed drain no longer owns input.
        // Only its original lease can be released here, even after a takeover.
        for session in &mut pool.sessions {
            if session.core.owns(broker::CLI_SENDER)
                && !server.has_cli_drain(session.id, session.core.control_epoch())
            {
                session.core.release(broker::CLI_SENDER)?;
            }
        }
        openers.reap();
        if let Some(target) = pool.pending_reload.take() {
            let result = if openers.pending() {
                Err(String::from(
                    "reload waits for external opener processes to finish",
                ))
            } else {
                pool.reload_checkpoint_ready(&server, &mut read_buffer)
                    .and_then(|(meta, terminals, fds)| {
                        crate::runtime_reload::execute(&target, meta, terminals, &fds)
                    })
            };
            // A successful exec never returns. Any preparation/exec error keeps
            // the existing Pool, windows, parser state and PTYs in this loop.
            server.finish_reload(result.map(|()| Value::Null));
        }
        if !pool.sessions.is_empty() || server.has_pending() {
            idle_since = os::milliseconds();
        }
        if pool.sessions.is_empty()
            && pool.reaping.is_empty()
            && pool.plugins.is_empty()
            && os::milliseconds().saturating_sub(idle_since) > 3000
        {
            return Ok(());
        }
        fds.clear();
        server.descriptors(&mut fds);
        for run in &pool.plugins {
            if let Some(fd) = run.job.descriptor() {
                fds.push(fd);
            }
        }
        if pool.watcher.fd() >= 0 {
            fds.push(os::PollFd {
                fd: pool.watcher.fd(),
                events: os::POLLIN,
                revents: 0,
            });
        }
        for (_, connection) in &pool.connections {
            connection.descriptors(&mut fds);
        }
        for session in &pool.sessions {
            if let Some(descriptor) = session
                .ui
                .as_ref()
                .filter(|ui| ui.open && ui.visible && ui.window.id != 0)
                .and_then(|ui| ui.window.renderer_failure_descriptor())
            {
                fds.push(descriptor);
            }
            if !session.core.eof() {
                fds.push(os::PollFd {
                    fd: session.core.pty.fd(),
                    events: os::POLLIN
                        | if session.core.input.pending() {
                            os::POLLOUT
                        } else {
                            0
                        },
                    revents: 0,
                });
            }
        }
        let mut timeout = deadline
            .into_iter()
            .chain(server.next_deadline())
            .chain(pool.connections.iter().filter_map(|(_, c)| c.deadline()))
            .min()
            .map(|end| end.saturating_sub(os::milliseconds()).min(5000) as c_int)
            .unwrap_or(5000);
        if pool.sessions.is_empty() {
            timeout = timeout.min(100);
        }
        if openers.pending() {
            timeout = timeout.min(50);
        }
        if pool.connections.iter().any(|(_, c)| c.pending()) {
            timeout = 0;
        }
        if unsafe { os::poll(fds.as_mut_ptr(), fds.len(), timeout) } < 0 && os::errno() != os::EINTR
        {
            return Err("session event loop poll failed".into());
        }
    }
}
