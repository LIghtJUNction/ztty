//! Desktop workspace arrangement and saved layout operations.
use super::{Pool, Workspace, pty_size, requests::local_directory};
use crate::{
    broker,
    json::{self, Value},
    layout::{Axis, Layout},
    os, workspace_state, x11,
};
use alloc::{format, string::String};

impl Pool {
    pub(super) fn workspace(&self, session: u64) -> Option<usize> {
        self.workspaces
            .iter()
            .position(|workspace| workspace.layout.contains(session))
    }

    pub(super) fn remove_from_layout(&mut self, id: u64) {
        if let Some(index) = self.workspace(id) {
            self.workspaces[index].layout.remove(id);
            if self.workspaces[index].layout.tabs.is_empty() {
                self.workspaces.remove(index);
            } else {
                let _ = self.arrange(index, false);
            }
        }
    }

    pub(super) fn ensure_workspace(&mut self, id: u64) -> Result<usize, String> {
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
        let window = x11::Container::new(ui.window.x11()?);
        ui.window.parent(Some(window.id));
        ui.workspace = Some(window.id);
        self.workspaces.push(Workspace {
            window,
            layout: Layout::new(id),
            title: String::new(),
            reveal_pending: false,
            focus_after_reveal: false,
        });
        Ok(self.workspaces.len() - 1)
    }

    pub(super) fn arrange(&mut self, index: usize, focus: bool) -> Result<(), String> {
        let workspace = &self.workspaces[index];
        let placements = workspace
            .layout
            .placements(workspace.window.width, workspace.window.height);
        let members = workspace.layout.panes();
        let active = workspace.layout.focused;
        self.workspaces[index].reveal_pending = true;
        self.workspaces[index].focus_after_reveal |= focus;
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
                if session.core.owns("local")
                    && !ui.read_only
                    && (cols != session.core.terminal.cols()
                        || rows != session.core.terminal.rows())
                {
                    let mut anchor = ui.capture_view_anchor(&session.core.terminal);
                    let resized = if session.core.eof() {
                        session
                            .core
                            .terminal
                            .resize_anchored(cols, rows, anchor.anchors_mut());
                        true
                    } else {
                        match session.core.resize_anchored(
                            pty_size(cols, rows, Some(&ui.window)),
                            anchor.anchors_mut(),
                        ) {
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
                        ui.restore_view_anchor(&session.core.terminal, anchor);
                    }
                }
                // Keep the outgoing tab's completed pixels on top until every
                // incoming pane has published its first complete frame.
                ui.window.stage_visible();
                ui.scroll_prompt = None;
                ui.force = true;
            } else {
                ui.scroll_prompt = None;
                // Native unmapping is committed by reveal_workspace after
                // paint. PTYs still drain while the old view covers the work.
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

    pub(super) fn reveal_workspace(&mut self, index: usize) {
        let workspace = &self.workspaces[index];
        if !workspace.reveal_pending {
            return;
        }
        let placements = workspace
            .layout
            .placements(workspace.window.width, workspace.window.height);
        if placements.iter().any(|(id, _)| {
            self.sessions
                .iter()
                .find(|session| session.id == *id)
                .and_then(|session| session.ui.as_ref())
                .is_none_or(|ui| {
                    !ui.visible
                        || ui.force
                        || ui.window.needs_repaint()
                        || !ui.window.x11().is_ok_and(|window| window.ready_to_reveal())
                })
        }) {
            return;
        }
        let members = workspace.layout.panes();
        let active = workspace.layout.focused;
        let focus = workspace.focus_after_reveal;
        for session in &mut self.sessions {
            if members.contains(&session.id)
                && let Some(ui) = &mut session.ui
                && !ui.visible
            {
                ui.window.visible(false);
            }
        }
        self.workspaces[index].reveal_pending = false;
        self.workspaces[index].focus_after_reveal = false;
        if focus
            && let Some(ui) = self
                .sessions
                .iter()
                .find(|session| session.id == active)
                .and_then(|session| session.ui.as_ref())
        {
            ui.window.raise();
        }
    }

    pub(super) fn group_layout(
        &self,
        target: u64,
        moved: u64,
        mode: &str,
    ) -> Result<Layout, String> {
        if target == moved {
            return Err("cannot put a session beside itself".into());
        }
        let axis = match mode {
            "tab" => None,
            "right" => Some(Axis::Vertical),
            "down" => Some(Axis::Horizontal),
            _ => return Err("layout must be tab, right or down".into()),
        };
        let mut layout = self
            .workspace(target)
            .map(|index| self.workspaces[index].layout.clone())
            .unwrap_or_else(|| Layout::new(target));
        layout.focus(target);
        if let Some(axis) = axis {
            layout.split(moved, axis)?;
        } else {
            layout.add_tab(moved)?;
        }
        Ok(layout)
    }

    pub(super) fn group(&mut self, target: u64, moved: u64, mode: &str) -> Result<(), String> {
        let layout = self.group_layout(target, moved, mode)?;
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
        let index = self.ensure_workspace(target)?;
        let parent = self.workspaces[index].window.id;
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

    pub(super) fn popout(&mut self, id: u64) -> Result<(), String> {
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

    pub(super) fn detach(&mut self, index: usize) -> Value {
        let session = &mut self.sessions[index];
        session.core.terminal.set_ui_host_enabled(false);
        session.persistent = true;
        let id = session.id;
        let had_view = session.ui.is_some();
        if let Some(ui) = session.ui.take() {
            let token = format!("attachment:{}", ui.attachment);
            let status = session
                .status()
                .unwrap_or_else(|| i32::from(ui.window.failed()));
            self.finished.push((token.clone(), status));
            self.completed.push_back((token, status));
            while self.completed.len() > 128 {
                self.completed.pop_front();
            }
            drop(ui);
        }
        self.remove_from_layout(id);
        if had_view {
            os::release_unused_memory();
        }
        self.sessions[index].info()
    }

    pub(super) fn related(&mut self, index: usize, mode: &str) -> Result<Value, String> {
        let session = &self.sessions[index];
        let mut request = json::object([
            ("op", Value::from("create")),
            ("origin", Value::from("local")),
            ("headless", Value::Bool(false)),
            (
                "_backend",
                Value::from(session.ui.as_ref().map_or("x11", |ui| ui.window.backend())),
            ),
            ("cols", Value::from(session.core.terminal.cols())),
            ("rows", Value::from(session.core.terminal.rows())),
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
        if let Some(profile) = &session.ssh_profile {
            let name = alloc::format!(
                "pane-{}-{}-{}",
                unsafe { os::getpid() },
                os::unix_milliseconds(),
                self.next
            );
            let (profile, command) = crate::mux::related_command(profile, &name)?;
            request.set("_ssh", profile);
            request.set(
                "_argv",
                Value::Array(
                    command
                        .iter()
                        .map(|arg| Value::from(broker::hex(arg.to_bytes())))
                        .collect(),
                ),
            );
        } else if let Some(cwd) = local_directory(session.core.terminal.working_directory()) {
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

    pub(super) fn save_layout(&self, index: usize) -> Result<Value, String> {
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

    pub(super) fn restore_layout(&mut self, value: &Value) -> Result<Value, String> {
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
        let mut window =
            x11::Container::new(self.sessions[active].ui.as_ref().unwrap().window.x11()?);
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
            reveal_pending: false,
            focus_after_reveal: false,
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
}
