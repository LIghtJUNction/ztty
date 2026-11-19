//! Broker request validation, session creation and input ownership.
use super::{Pool, Session, Ui, pty_size};
use crate::{
    Appearance,
    broker::{self, Reply},
    display::Window,
    json::{self, Value},
    os, session_control,
    session_core::SessionCore,
    working_directory::WorkingDirectory,
};
use alloc::{ffi::CString, format, string::String, vec::Vec};
use core::ffi::CStr;
use zterm::Terminal;

impl Pool {
    pub(super) fn selector(&self, request: &Value) -> Result<usize, String> {
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

    pub(super) fn create(&mut self, request: &Value) -> Result<Value, String> {
        if request.get("_backend").and_then(Value::as_str) == Some("wayland")
            && request.get("target").is_some()
        {
            return Err(
                "Wayland workspaces are not available; use independent session windows".into(),
            );
        }
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
            let c = self.connection(
                &env,
                request.get("_backend").and_then(Value::as_str) == Some("wayland"),
            )?;
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
        let mut display_fds = Vec::new();
        for (_, c) in &self.connections {
            c.descriptors(&mut display_fds);
        }
        let close: Vec<i32> = display_fds.iter().map(|p| p.fd).collect();
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
            terminal.set_ui_host_enabled(true);
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
            core: SessionCore::new(pty, terminal, input_owner, 8 * 1024 * 1024),
            ui,
            persistent: origin != "local",
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

    pub(super) fn retire(&mut self, index: usize) -> Value {
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
        let status = session
            .status()
            .unwrap_or_else(|| i32::from(session.ui.as_ref().is_some_and(|ui| ui.window.failed())));
        if let Some(ui) = &session.ui {
            let token = format!("attachment:{}", ui.attachment);
            self.finished.push((token.clone(), status));
            self.completed.push_back((token, status));
        }
        while self.completed.len() > 128 {
            self.completed.pop_front();
        }
        if session.core.pty.status().is_none() {
            let _ = session.core.pty.hangup();
            self.reaping
                .push((session.core.pty.pid(), os::milliseconds()));
        }
        drop(session);
        self.remove_from_layout(result.get("id").and_then(Value::as_u64).unwrap_or(0));
        result
    }

    pub(super) fn control(&mut self, index: usize, request: &Value) -> Result<Value, String> {
        let actor = actor(request)?;
        let session = &mut self.sessions[index];
        let action = request
            .get("action")
            .and_then(Value::as_str)
            .ok_or("control action is required")?;
        match action {
            "acquire" => {
                let takeover = request.get("takeover").and_then(Value::as_bool) == Some(true);
                if session.core.acquire(actor, takeover)?
                    && let Some(ui) = &mut session.ui
                {
                    ui.cancel_input();
                }
                if actor == "local"
                    && let Some(ui) = &mut session.ui
                {
                    ui.read_only = false;
                }
            }
            "release" => {
                session.core.release(actor)?;
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

    pub(super) fn handle(&mut self, request: &Value) -> Result<Reply, String> {
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
        let session = &self.sessions[index];
        if let Some(expected) = request.get("expected_session")
            && !crate::session_identity::ExpectedSession::parse(expected)?.matches(
                &session.uri,
                session.id,
                session.created_at,
                unsafe { os::getpid() } as u32,
            )
        {
            return Err(if op == "read" {
                "session identity changed; start a new read"
            } else {
                "session identity changed; refresh session metadata"
            }
            .into());
        }
        if matches!(op, "write" | "resize" | "close" | "rename")
            && !self.sessions[index].core.owns(actor(request)?)
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
                    .core
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
                let terminal = &self.sessions[index].core.terminal;
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
            "read" => self.sessions[index].read(request),
            "write" => {
                let session = &mut self.sessions[index];
                if session.core.eof() || session.status().is_some() {
                    return Err("session has exited".into());
                }
                let data = request
                    .get("data")
                    .and_then(Value::as_str)
                    .ok_or("data must be a string")?;
                if data.len() > 65536 {
                    return Err("write data exceeds 65536 bytes".into());
                }
                if !session.core.input.push(data.as_bytes()) {
                    return Err("session input queue is full".into());
                }
                if !data.is_empty()
                    && let Some(ui) = &mut session.ui
                {
                    ui.repaint.input_sent();
                }
                let _ = session.core.flush_input();
                Ok(Reply::Value(json::object([
                    ("written", Value::from(data.len())),
                    ("uri", Value::from(session.uri.clone())),
                ])))
            }
            "resize" => {
                let cols = bounded(request, "cols", 120, 2, 512)?;
                let rows = bounded(request, "rows", 32, 2, 256)?;
                let session = &mut self.sessions[index];
                let mut anchor = session
                    .ui
                    .as_ref()
                    .map(|ui| ui.capture_view_anchor(&session.core.terminal))
                    .unwrap_or_else(|| {
                        ztty::selection::ViewAnchor::capture(&session.core.terminal, 0, None)
                    });
                session
                    .core
                    .resize_anchored(
                        pty_size(cols, rows, session.ui.as_ref().map(|ui| &ui.window)),
                        anchor.anchors_mut(),
                    )
                    .map_err(|e| format!("resize failed: {e}"))?;
                if let Some(ui) = &mut session.ui {
                    ui.window.resize_grid(cols, rows);
                    ui.force = true;
                    ui.restore_view_anchor(&session.core.terminal, anchor);
                    ui.scroll_prompt = None;
                }
                Ok(Reply::Value(session.info()))
            }
            "attach" => {
                if let Some(requested) = request.get("_backend").and_then(Value::as_str)
                    && self.sessions[index]
                        .ui
                        .as_ref()
                        .is_some_and(|ui| ui.window.backend() != requested)
                {
                    return Err("session already has a window on another backend; detach it before attaching a different backend".into());
                }
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
                    let observing = read_only || !self.sessions[index].core.owns("local");
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
                let connection = self.connection(
                    &env,
                    request.get("_backend").and_then(Value::as_str) == Some("wayland"),
                )?;
                self.attach_view(index, read_only, &connection, request.get("_appearance"))?;
                Ok(Reply::Value(self.sessions[index].info()))
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
}

pub(super) fn bounded(
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

pub(super) fn environment(request: &Value) -> Result<Vec<CString>, String> {
    match request.get("_env").and_then(Value::as_array) {
        None => Ok(os::environment()),
        Some(entries) => entries
            .iter()
            .map(|v| broker::unhex(v.as_str().ok_or("invalid process environment")?))
            .collect(),
    }
}

pub(super) fn actor(request: &Value) -> Result<&str, String> {
    match request.get("_actor") {
        None => Ok("local"),
        Some(value) => value
            .as_str()
            .filter(|s| session_control::valid_actor(s))
            .ok_or_else(|| String::from("invalid control identity")),
    }
}

pub(super) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

pub(super) fn local_directory(uri: &str) -> Option<String> {
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
