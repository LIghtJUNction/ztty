//! Checkpoint the common shell/TUI session model, then reconstruct native views.
//! Unsupported transient resources reject preparation before the old loop changes.
use super::*;
use crate::runtime_reload::{boolean, bytes, descriptor, list, number, text};
use crate::workspace_state;
use ztty::session_core::CoreCheckpoint;

type ReloadCheckpoint = (Value, Vec<Vec<u8>>, Vec<i32>);

fn last_view_backend(value: &Value) -> Result<Option<Backend>, String> {
    let previous = match value.get("last_view_backend") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_str().ok_or("invalid reload last view backend")?),
    };
    let current = value
        .get("ui")
        .filter(|ui| **ui != Value::Null)
        .map(|ui| text(ui, "backend"))
        .transpose()?;
    Backend::from_checkpoint(previous, current).map_err(String::from)
}

fn optional_text(value: &Value, key: &str) -> Result<Option<String>, String> {
    match value.get(key) {
        Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        _ => Err(format!("invalid reload {key}")),
    }
}
fn raw_status(value: &Value) -> Result<Option<i32>, String> {
    match value.get("status") {
        Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| i32::try_from(n).ok())
            .filter(|n| *n <= 65535)
            .map(Some)
            .ok_or_else(|| "invalid reload child status".into()),
        None => Err("missing reload child status".into()),
    }
}
fn process_id(value: &Value) -> Result<i32, String> {
    i32::try_from(number(value, "pid")?)
        .ok()
        .filter(|pid| *pid > 0)
        .ok_or_else(|| "invalid reload child process".into())
}
fn core_value(core: &SessionCore) -> Value {
    let state = core.checkpoint();
    json::object([
        ("input", broker::hex(&state.input).into()),
        ("input_limit", state.input_limit.into()),
        ("output", broker::hex(&state.output).into()),
        ("output_end", state.output_end.into()),
        ("owner", state.owner.map(Value::from).unwrap_or(Value::Null)),
        ("eof", state.eof.into()),
        ("control_epoch", state.control_epoch.into()),
    ])
}
fn read_core(value: &Value) -> Result<CoreCheckpoint, String> {
    let state = CoreCheckpoint {
        input: bytes(value, "input", 8 * 1024 * 1024)?,
        input_limit: number(value, "input_limit")? as usize,
        output: bytes(value, "output", 256 * 1024)?,
        output_end: number(value, "output_end")?,
        owner: optional_text(value, "owner")?,
        eof: boolean(value, "eof")?,
        control_epoch: number(value, "control_epoch")?,
    };
    state.validate().map_err(String::from)?;
    Ok(state)
}
fn environment(value: &Value) -> Result<Vec<CString>, String> {
    let values = list(value, "env")?;
    if values.len() > 4096 {
        return Err("reload environment is too large".into());
    }
    values
        .iter()
        .map(|v| broker::unhex(v.as_str().ok_or("invalid reload environment")?))
        .collect()
}
fn point(value: &Value) -> Result<(usize, usize), String> {
    let p = value
        .as_array()
        .filter(|p| p.len() == 2)
        .ok_or("invalid reload selection point")?;
    let x = p[0]
        .as_u64()
        .filter(|n| *n <= 1024)
        .ok_or("invalid reload selection column")?;
    let y = p[1]
        .as_u64()
        .filter(|n| *n <= 131072)
        .ok_or("invalid reload selection row")?;
    Ok((x as usize, y as usize))
}
fn point_value(p: (usize, usize)) -> Value {
    Value::Array(alloc::vec![p.0.into(), p.1.into()])
}

impl Ui {
    fn reload_value(&self) -> Result<Value, String> {
        if self.window.backend() != "x11" {
            return Err("runtime reload currently requires X11 views; Wayland session kept".into());
        }
        if !self.open
            || self.search.is_some()
            || self.palette.is_some()
            || self.clipboard_menu.is_some()
            || self.osc_clipboard.is_some()
            || self.pending_action.is_some()
            || self.plugin_running
            || self.window.paste_pending()
        {
            return Err(
                "finish the active overlay, paste or extension before runtime reload".into(),
            );
        }
        let held: Vec<u8> = self.held.iter().flat_map(|n| n.to_le_bytes()).collect();
        let clipboards = self.window.x11().map_err(String::from)?.reload_clipboards();
        Ok(json::object([
            ("backend", "x11".into()),
            ("width", self.window.width.into()),
            ("height", self.window.height.into()),
            ("focused", self.window.focused.into()),
            ("visible", self.visible.into()),
            ("read_only", self.read_only.into()),
            ("owns_child", self.owns_child.into()),
            ("attachment", self.attachment.into()),
            ("history", self.history.into()),
            ("new_output", self.new_output.into()),
            (
                "command",
                self.command.map(Value::from).unwrap_or(Value::Null),
            ),
            (
                "selection",
                self.selection
                    .map(|s| {
                        json::object([
                            ("start", point_value(s.start)),
                            ("end", point_value(s.end)),
                            ("dragging", s.dragging.into()),
                        ])
                    })
                    .unwrap_or(Value::Null),
            ),
            (
                "selection_unit",
                Value::from(match self.selection_unit {
                    ztty::selection::Unit::Cell => "cell",
                    ztty::selection::Unit::Word => "word",
                    ztty::selection::Unit::Line => "line",
                }),
            ),
            ("anchor_start", point_value(self.selection_anchor.0)),
            ("anchor_end", point_value(self.selection_anchor.1)),
            ("held", broker::hex(&held).into()),
            (
                "intercepted",
                broker::hex(&self.intercepted.map(u8::from)).into(),
            ),
            (
                "local_buttons",
                broker::hex(&self.local_buttons.map(u8::from)).into(),
            ),
            (
                "swallowed_buttons",
                broker::hex(&self.swallowed_buttons.map(u8::from)).into(),
            ),
            ("reported_buttons", u64::from(self.reported_buttons).into()),
            (
                "sync_since",
                self.sync_since.map(Value::from).unwrap_or(Value::Null),
            ),
            (
                "clipboards",
                Value::Array(
                    clipboards
                        .into_iter()
                        .map(|s| s.map(Value::from).unwrap_or(Value::Null))
                        .collect(),
                ),
            ),
        ]))
    }
}

fn validate_ui(value: &Value, terminal: &zterm::Terminal) -> Result<(), String> {
    if text(value, "backend")? != "x11" {
        return Err("unsupported reload window backend".into());
    }
    for key in ["width", "height"] {
        if !(1..=32767).contains(&number(value, key)?) {
            return Err("invalid reload window dimensions".into());
        }
    }
    if number(value, "width")?.saturating_mul(number(value, "height")?) > 64 * 1024 * 1024 {
        return Err("reload window pixel budget exceeded".into());
    }
    for key in [
        "focused",
        "visible",
        "read_only",
        "owns_child",
        "new_output",
    ] {
        boolean(value, key)?;
    }
    number(value, "attachment")?;
    if number(value, "history")? > terminal.history_len() as u64 {
        return Err("invalid reload history offset".into());
    }
    if let Some(selection) = value.get("selection").filter(|v| **v != Value::Null) {
        point(selection.get("start").ok_or("missing reload selection")?)?;
        point(selection.get("end").ok_or("missing reload selection")?)?;
        boolean(selection, "dragging")?;
    }
    if !matches!(text(value, "selection_unit")?, "cell" | "word" | "line") {
        return Err("invalid reload selection unit".into());
    }
    point(value.get("anchor_start").ok_or("missing reload anchor")?)?;
    point(value.get("anchor_end").ok_or("missing reload anchor")?)?;
    for (key, length) in [
        ("held", 2048),
        ("intercepted", 256),
        ("local_buttons", 12),
        ("swallowed_buttons", 12),
    ] {
        let data = bytes(value, key, length)?;
        if data.len() != length || (key != "held" && data.iter().any(|b| *b > 1)) {
            return Err("invalid reload input state".into());
        }
    }
    if number(value, "reported_buttons")? > u16::MAX as u64 {
        return Err("invalid reload buttons".into());
    }
    let clipboards = list(value, "clipboards")?;
    if clipboards.len() != 2
        || clipboards
            .iter()
            .any(|v| *v != Value::Null && v.as_str().is_none_or(|s| s.len() > 4 * 1024 * 1024))
    {
        return Err("invalid reload clipboard state".into());
    }
    Ok(())
}

impl Pool {
    pub(super) fn reload_checkpoint_ready(
        &mut self,
        server: &broker::Server,
        buffer: &mut [u8],
    ) -> Result<ReloadCheckpoint, String> {
        let until = os::milliseconds().saturating_add(250);
        loop {
            match self.reload_checkpoint(server) {
                Err(error)
                    if (error.contains(
                        "terminal parser has pending input; retry after a complete sequence",
                    ) || error.contains(
                        "terminal DCS query is incomplete; retry after a complete sequence",
                    )) && os::milliseconds() < until =>
                {
                    // Read only into the still-live old model, then export a
                    // fresh snapshot. Once export succeeds there is no further
                    // PTY read before exec, so a successful cut has no gap.
                    for session in &mut self.sessions {
                        session.pump(buffer);
                    }
                    let mut fds: Vec<os::PollFd> = self
                        .sessions
                        .iter()
                        .filter(|s| !s.core.eof())
                        .map(|s| os::PollFd {
                            fd: s.core.pty.fd(),
                            events: os::POLLIN,
                            revents: 0,
                        })
                        .collect();
                    unsafe {
                        os::poll(fds.as_mut_ptr(), fds.len(), 5);
                    }
                }
                result => return result,
            }
        }
    }

    pub(super) fn reload_checkpoint(
        &self,
        server: &broker::Server,
    ) -> Result<ReloadCheckpoint, String> {
        if !self.plugins.is_empty() || !self.reaping.is_empty() || !self.native_gestures.is_empty()
        {
            return Err("wait for active extension or child cleanup before runtime reload".into());
        }
        if self.sessions.len() > 128 {
            return Err("runtime reload supports at most 128 sessions".into());
        }
        let mut sessions = Vec::new();
        let mut terminals = Vec::new();
        let mut terminal_bytes = 0usize;
        for session in &self.sessions {
            let terminal = session
                .core
                .terminal
                .checkpoint()
                .map_err(|e| format!("session {}: {e}; original session kept", session.name))?;
            terminal_bytes = terminal_bytes
                .checked_add(terminal.len())
                .ok_or("reload checkpoint size overflow")?;
            if terminal_bytes > 128 * 1024 * 1024 {
                return Err("reload terminal state exceeds its memory budget".into());
            }
            terminals.push(terminal);
            sessions.push(json::object([
                ("id", session.id.into()),
                ("created_at", session.created_at.into()),
                ("uri", session.uri.clone().into()),
                ("name", session.name.clone().into()),
                ("origin", session.origin.clone().into()),
                ("ssh", session.ssh_profile.clone().unwrap_or(Value::Null)),
                ("appearance", session.appearance.to_json()),
                ("core", core_value(&session.core)),
                ("fd", session.core.pty.fd().into()),
                ("pid", session.core.pty.pid().into()),
                (
                    "status",
                    session
                        .core
                        .pty
                        .status()
                        .map(|s| Value::from(s.raw()))
                        .unwrap_or(Value::Null),
                ),
                ("persistent", session.persistent.into()),
                (
                    "env",
                    Value::Array(
                        session
                            .launch_env
                            .iter()
                            .map(|s| Value::from(broker::hex(s.to_bytes())))
                            .collect(),
                    ),
                ),
                ("cwd", broker::hex(session.initial_cwd.to_bytes()).into()),
                (
                    "config",
                    session
                        .config_path
                        .as_ref()
                        .map(|p| Value::from(broker::hex(p.to_bytes())))
                        .unwrap_or(Value::Null),
                ),
                ("base_font", session.base_font.clone().into()),
                (
                    "last_view_backend",
                    session
                        .last_view_backend
                        .map_or(Value::Null, |backend| Value::from(backend.name())),
                ),
                (
                    "ui",
                    session
                        .ui
                        .as_ref()
                        .map(Ui::reload_value)
                        .transpose()?
                        .unwrap_or(Value::Null),
                ),
            ]));
        }
        let layouts: Vec<Value> = self
            .workspaces
            .iter()
            .map(|workspace| {
                let index = self
                    .sessions
                    .iter()
                    .position(|s| s.id == workspace.layout.focused)
                    .ok_or("reload layout lost its focused session")?;
                self.save_layout(index)
            })
            .collect::<Result<_, String>>()?;
        let meta = json::object([
            ("broker_pid", (unsafe { os::getpid() } as u64).into()),
            ("next", self.next.into()),
            ("next_attachment", self.next_attachment.into()),
            (
                "shortcuts",
                broker::hex(&self.shortcuts.map(u8::from)).into(),
            ),
            (
                "mouse_shortcuts",
                broker::hex(&self.mouse_shortcuts.map(u8::from)).into(),
            ),
            ("sessions", Value::Array(sessions)),
            ("layouts", Value::Array(layouts)),
            (
                "completed",
                Value::Array(
                    self.completed
                        .iter()
                        .map(|(key, status)| {
                            json::object([
                                ("key", key.clone().into()),
                                ("status", (*status).into()),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("server", server.checkpoint_reload()),
            ("gpu_workers", crate::gpu_worker::checkpoint()),
        ]);
        let fds = reload_fds(&meta)?;
        Ok((meta, terminals, fds))
    }
}

pub(crate) fn reload_fds(meta: &Value) -> Result<Vec<i32>, String> {
    let mut fds =
        broker::Server::validate_reload(meta.get("server").ok_or("missing reload server")?)?;
    for session in list(meta, "sessions")? {
        let fd = descriptor(session, "fd")?;
        if fds.contains(&fd) {
            return Err("duplicate reload descriptor ownership".into());
        }
        fds.push(fd);
    }
    Ok(fds)
}

pub(crate) fn validate_reload(meta: &Value, terminals: &[zterm::Terminal]) -> Result<(), String> {
    let sessions = list(meta, "sessions")?;
    if sessions.len() != terminals.len() || sessions.len() > 128 {
        return Err("reload terminal count mismatch".into());
    }
    let broker_pid = number(meta, "broker_pid")?;
    if broker_pid == 0 || broker_pid > i32::MAX as u64 {
        return Err("invalid reload broker identity".into());
    }
    if number(meta, "next")? == 0 || number(meta, "next_attachment")? == 0 {
        return Err("invalid reload identity counters".into());
    }
    for (key, length) in [("shortcuts", 256), ("mouse_shortcuts", 12)] {
        let raw = bytes(meta, key, length)?;
        if raw.len() != length || raw.iter().any(|b| *b > 1) {
            return Err("invalid reload shortcut state".into());
        }
    }
    let mut ids = Vec::new();
    let mut uris = Vec::new();
    let mut attachments = Vec::new();
    for (session, terminal) in sessions.iter().zip(terminals) {
        let id = number(session, "id")?;
        if id == 0 || id >= number(meta, "next")? || ids.contains(&id) {
            return Err("invalid reload session identity".into());
        }
        ids.push(id);
        number(session, "created_at")?;
        if !requests::valid_name(text(session, "name")?) || text(session, "uri")?.len() > 2048 {
            return Err("invalid reload session name".into());
        }
        let uri = text(session, "uri")?;
        if uris.contains(&uri) {
            return Err("duplicate reload session URI".into());
        }
        uris.push(uri);
        text(session, "origin")?;
        if let Some(profile) = session.get("ssh").filter(|v| **v != Value::Null) {
            crate::mux::validate_profile(profile)?;
        }
        text(session, "base_font")?;
        process_id(session)?;
        raw_status(session)?;
        boolean(session, "persistent")?;
        last_view_backend(session)?;
        Appearance::from_json(
            session
                .get("appearance")
                .ok_or("missing reload appearance")?,
        )
        .map_err(String::from)?;
        read_core(session.get("core").ok_or("missing reload core")?)?;
        environment(session)?;
        broker::unhex(text(session, "cwd")?)?;
        if let Some(config) = optional_text(session, "config")? {
            broker::unhex(&config)?;
        }
        if let Some(ui) = session.get("ui").filter(|v| **v != Value::Null) {
            validate_ui(ui, terminal)?;
            let attachment = number(ui, "attachment")?;
            if attachment == 0
                || attachment >= number(meta, "next_attachment")?
                || attachments.contains(&attachment)
            {
                return Err("invalid reload attachment identity".into());
            }
            attachments.push(attachment);
        }
    }
    let mut placed = Vec::new();
    for layout in list(meta, "layouts")? {
        let snapshot = workspace_state::import(layout, |uri, id, created, pid| {
            sessions
                .iter()
                .find(|s| {
                    s.get("id").and_then(Value::as_u64) == Some(id)
                        && s.get("uri").and_then(Value::as_str) == Some(uri)
                        && s.get("created_at").and_then(Value::as_u64) == Some(created)
                        && pid as u64 == broker_pid
                })
                .map(|_| id)
        })
        .map_err(String::from)?;
        for id in snapshot.layout.panes() {
            if placed.contains(&id)
                || sessions
                    .iter()
                    .find(|s| s.get("id").and_then(Value::as_u64) == Some(id))
                    .and_then(|s| s.get("ui"))
                    .is_none_or(|ui| *ui == Value::Null)
            {
                return Err("reload layouts duplicate or lack a session view".into());
            }
            placed.push(id);
        }
    }
    for completed in list(meta, "completed")? {
        text(completed, "key")?;
        completed
            .get("status")
            .and_then(Value::as_i64)
            .and_then(|n| i32::try_from(n).ok())
            .ok_or("invalid completed reload status")?;
    }
    crate::gpu_worker::validate_checkpoint(meta.get("gpu_workers"))?;
    reload_fds(meta)?;
    Ok(())
}

impl Pool {
    fn restore_reload_views(&mut self, meta: &Value) -> Result<(), String> {
        for (index, session) in list(meta, "sessions")?.iter().enumerate() {
            let Some(saved) = session.get("ui").filter(|v| **v != Value::Null) else {
                continue;
            };
            let env = self.sessions[index].launch_env.clone();
            let connection = self.connection(&env, false)?;
            let session = &mut self.sessions[index];
            let window = Window::new_view(
                session.core.terminal.cols(),
                session.core.terminal.rows(),
                &session.appearance,
                &connection,
                false,
            )
            .map_err(String::from)?;
            let mut ui = Ui::new(window, number(saved, "attachment")?);
            ui.window.resize(
                number(saved, "width")? as i32,
                number(saved, "height")? as i32,
            );
            ui.read_only = boolean(saved, "read_only")?;
            ui.owns_child = boolean(saved, "owns_child")?;
            ui.visible = boolean(saved, "visible")?;
            ui.history = number(saved, "history")? as usize;
            ui.new_output = boolean(saved, "new_output")?;
            ui.command = saved.get("command").and_then(Value::as_u64);
            ui.selection = match saved.get("selection").filter(|v| **v != Value::Null) {
                Some(s) => Some(Selection {
                    start: point(s.get("start").ok_or("missing selection")?)?,
                    end: point(s.get("end").ok_or("missing selection")?)?,
                    dragging: boolean(s, "dragging")?,
                }),
                None => None,
            };
            ui.selection_unit = match text(saved, "selection_unit")? {
                "word" => ztty::selection::Unit::Word,
                "line" => ztty::selection::Unit::Line,
                _ => ztty::selection::Unit::Cell,
            };
            ui.selection_anchor = (
                point(
                    saved
                        .get("anchor_start")
                        .ok_or("missing selection anchor")?,
                )?,
                point(saved.get("anchor_end").ok_or("missing selection anchor")?)?,
            );
            let held = bytes(saved, "held", 2048)?;
            for (target, raw) in ui.held.iter_mut().zip(held.as_chunks::<8>().0.iter()) {
                *target = u64::from_le_bytes(*raw);
            }
            for (target, raw) in ui
                .intercepted
                .iter_mut()
                .zip(bytes(saved, "intercepted", 256)?)
            {
                *target = raw != 0;
            }
            for (target, raw) in ui
                .local_buttons
                .iter_mut()
                .zip(bytes(saved, "local_buttons", 12)?)
            {
                *target = raw != 0;
            }
            for (target, raw) in
                ui.swallowed_buttons
                    .iter_mut()
                    .zip(bytes(saved, "swallowed_buttons", 12)?)
            {
                *target = raw != 0;
            }
            ui.reported_buttons = number(saved, "reported_buttons")? as u16;
            ui.sync_since = saved.get("sync_since").and_then(Value::as_u64);
            for (primary, clipboard) in [false, true].into_iter().zip(list(saved, "clipboards")?) {
                if let Some(contents) = clipboard.as_str() {
                    // Do not replace a selection another application acquired
                    // while the native windows were being recreated.
                    if !ui.window.clipboard_available(primary) {
                        ui.window.copy(contents.into(), primary);
                    }
                }
            }
            ui.window.visible(ui.visible);
            session.core.terminal.mark_all_dirty();
            session.last_view_backend = Some(Backend::X11);
            session.ui = Some(ui);
        }
        for layout in list(meta, "layouts")? {
            self.restore_layout(layout)?;
        }
        for (index, session) in list(meta, "sessions")?.iter().enumerate() {
            if session
                .get("ui")
                .and_then(|s| s.get("focused"))
                .and_then(Value::as_bool)
                == Some(true)
                && let Some(ui) = &self.sessions[index].ui
            {
                ui.window.raise();
            }
        }
        Ok(())
    }
}

pub(crate) fn serve_reloaded(meta: Value, terminals: Vec<zterm::Terminal>) -> Result<(), String> {
    validate_reload(&meta, &terminals)?;
    let mut server = unsafe {
        broker::Server::restore_reload(meta.get("server").ok_or("missing reload server")?)?
    };
    let mut pool = Pool::new();
    pool.next = number(&meta, "next")?;
    pool.next_attachment = number(&meta, "next_attachment")?;
    for (target, raw) in pool
        .shortcuts
        .iter_mut()
        .zip(bytes(&meta, "shortcuts", 256)?)
    {
        *target = raw != 0;
    }
    for (target, raw) in pool
        .mouse_shortcuts
        .iter_mut()
        .zip(bytes(&meta, "mouse_shortcuts", 12)?)
    {
        *target = raw != 0;
    }
    for (saved, terminal) in list(&meta, "sessions")?.iter().zip(terminals) {
        let core = read_core(saved.get("core").ok_or("missing reload core")?)?;
        let pty = unsafe {
            zpty::Pty::from_raw_parts(
                descriptor(saved, "fd")?,
                process_id(saved)?,
                raw_status(saved)?,
            )
        };
        let session = Session {
            ssh_profile: saved.get("ssh").filter(|v| **v != Value::Null).cloned(),
            id: number(saved, "id")?,
            created_at: number(saved, "created_at")?,
            uri: text(saved, "uri")?.into(),
            name: text(saved, "name")?.into(),
            origin: text(saved, "origin")?.into(),
            appearance: Appearance::from_json(saved.get("appearance").ok_or("missing appearance")?)
                .map_err(String::from)?,
            core: SessionCore::restore_checkpoint(pty, terminal, core),
            ui: None,
            last_view_backend: last_view_backend(saved)?,
            persistent: boolean(saved, "persistent")?,
            launch_env: environment(saved)?,
            initial_cwd: broker::unhex(text(saved, "cwd")?)?,
            config_path: optional_text(saved, "config")?
                .map(|s| broker::unhex(&s))
                .transpose()?,
            base_font: text(saved, "base_font")?.into(),
        };
        if let Some(path) = &session.config_path {
            pool.watcher.watch(path);
        }
        pool.sessions.push(session);
    }
    for saved in list(&meta, "completed")? {
        pool.completed.push_back((
            text(saved, "key")?.into(),
            saved
                .get("status")
                .and_then(Value::as_i64)
                .ok_or("missing status")? as i32,
        ));
    }
    crate::gpu_worker::restore_children(meta.get("gpu_workers"));
    let window_result = pool.restore_reload_views(&meta);
    let mut result = json::object([
        ("reloaded", true.into()),
        ("broker_pid", (unsafe { os::getpid() }).into()),
        ("sessions", pool.sessions.len().into()),
        ("version", env!("CARGO_PKG_VERSION").into()),
    ]);
    if let Err(error) = window_result {
        // A native display can disappear after validation. Keep every PTY and
        // model alive for attach/recovery instead of dropping the restored pool.
        for session in &mut pool.sessions {
            session.persistent = true;
        }
        result.set("window_error", Value::from(error));
    }
    server.finish_reload(Ok(result));
    drop(meta);
    serve_loop(server, pool)
}
