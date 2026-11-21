//! Session metadata, output pages and bounded deferred input/output waits.
use super::{Session, requests::bounded};
use crate::{
    broker::{InputDrain, ReadUntil, ReadWait, Reply},
    json::{self, Value},
    os,
};
use alloc::string::String;
use ztty::output_tail::OutputChunk;

impl Session {
    pub(super) fn info(&self) -> Value {
        let mut value = json::object([
            ("id", Value::from(self.id)),
            ("created_at", Value::from(self.created_at)),
            ("uri", Value::from(self.uri.clone())),
            ("name", Value::from(self.name.clone())),
            ("origin", Value::from(self.origin.clone())),
            ("pid", Value::from(self.core.pty.pid())),
            ("broker_pid", Value::from(unsafe { os::getpid() })),
            ("cols", Value::from(self.core.terminal.cols())),
            ("rows", Value::from(self.core.terminal.rows())),
            ("attached", Value::Bool(self.ui.is_some())),
            (
                "backend",
                self.ui
                    .as_ref()
                    .map_or(Value::Null, |ui| Value::from(ui.window.backend())),
            ),
            (
                "last_view_backend",
                self.last_view_backend
                    .map_or(Value::Null, |backend| Value::from(backend.name())),
            ),
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
                Value::Bool(!self.core.eof() && self.core.pty.status().is_none()),
            ),
            (
                "exit_code",
                self.status().map(Value::from).unwrap_or(Value::Null),
            ),
            ("output_eof", Value::Bool(self.core.eof())),
            ("control_epoch", self.core.control_epoch().into()),
            ("title", Value::from(self.core.terminal.title())),
            (
                "input_owner",
                self.core.owner().map(Value::from).unwrap_or(Value::Null),
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
        ]);
        if let Some(profile) = &self.ssh_profile {
            value.set(
                "remote",
                json::object([
                    (
                        "destination",
                        profile.get("destination").cloned().unwrap_or(Value::Null),
                    ),
                    (
                        "session",
                        profile.get("session").cloned().unwrap_or(Value::Null),
                    ),
                ]),
            );
        }
        value
    }
    pub(super) fn read(&self, request: &Value) -> Result<Reply, String> {
        let cursor = match request.get("cursor") {
            None => 0,
            Some(v) => v
                .as_u64()
                .ok_or("cursor must be a nonnegative byte offset")?,
        };
        let max = match request.get("max_bytes") {
            None => None,
            Some(v) => Some(v.as_u64().ok_or("max_bytes must be 1..65536")?),
        };
        let wait_ms = bounded(request, "wait_ms", 0, 0, 30000)? as u64;
        let until = ReadUntil::parse(request.get("_until"))?;
        let chunk = self.core.read_output(cursor, max)?;
        if wait_ms > 0 && !self.read_ready(until, &chunk) {
            return Ok(Reply::ReadWait(ReadWait {
                session_id: self.id,
                until,
                cursor,
                max_bytes: max,
                deadline: os::milliseconds() + wait_ms,
                watch_revision: None,
                input_drain: None,
            }));
        }
        Ok(Reply::Value(
            self.read_value(chunk, (wait_ms > 0).then_some(false)),
        ))
    }

    fn read_ready(&self, until: Option<ReadUntil>, chunk: &OutputChunk) -> bool {
        // has_more also includes an incomplete UTF-8 suffix. Keep waiting for
        // its continuation instead of returning an empty page in a busy loop.
        match until.unwrap_or(ReadUntil::Output) {
            ReadUntil::Output => !chunk.bytes.is_empty() || chunk.truncated || self.core.eof(),
            ReadUntil::Eof => self.core.eof(),
            ReadUntil::Exit => self.core.eof() && self.status().is_some(),
        }
    }

    pub(super) fn wait_output(&self, wait: &ReadWait) -> Result<Option<Value>, String> {
        if let Some(drain) = &wait.input_drain {
            if !self.core.owns(&drain.actor) || self.core.control_epoch() != drain.control_epoch {
                return Err("input control changed while draining pending input".into());
            }
            if !self.core.input.pending() {
                return Ok(Some(json::object([
                    ("written", drain.written.into()),
                    ("uri", self.uri.as_str().into()),
                    ("drained", true.into()),
                ])));
            }
            if self.core.eof() || self.status().is_some() {
                return Err("session exited before pending input reached the PTY".into());
            }
            if os::milliseconds() >= wait.deadline {
                return Err(
                    "terminal input drain timed out; pending bytes were not accepted by the PTY"
                        .into(),
                );
            }
            return Ok(None);
        }
        if let Some((revision, control_epoch)) = wait.watch_revision {
            return Ok((self.frame_changed(revision, control_epoch)
                || os::milliseconds() >= wait.deadline)
                .then(|| self.frame_revision()));
        }
        // EOF/exit waits can ignore unrelated output without copying a retained page.
        if matches!(wait.until, Some(ReadUntil::Eof | ReadUntil::Exit))
            && !self.core.eof()
            && os::milliseconds() < wait.deadline
        {
            return Ok(None);
        }
        let chunk = self.core.read_output(wait.cursor, wait.max_bytes)?;
        let ready = self.read_ready(wait.until, &chunk);
        if ready || os::milliseconds() >= wait.deadline {
            Ok(Some(self.read_value(chunk, Some(!ready))))
        } else {
            Ok(None)
        }
    }

    /// Ordinary writes acknowledge queuing. Mux writes opt into this deferred
    /// reply so they apply backpressure and can safely release input control.
    pub(super) fn drain_input(
        &self,
        actor: &str,
        written: usize,
        wait_ms: u64,
    ) -> Result<Reply, String> {
        let wait = ReadWait {
            session_id: self.id,
            until: None,
            cursor: 0,
            max_bytes: None,
            deadline: os::milliseconds().saturating_add(wait_ms),
            watch_revision: None,
            input_drain: Some(InputDrain {
                actor: actor.into(),
                control_epoch: self.core.control_epoch(),
                written,
            }),
        };
        match self.wait_output(&wait)? {
            Some(value) => Ok(Reply::Value(value)),
            None => Ok(Reply::ReadWait(wait)),
        }
    }

    pub(super) fn read_value(&self, chunk: OutputChunk, timed_out: Option<bool>) -> Value {
        let mut screen = String::new();
        let mut clipped = false;
        'rows: for y in 0..self.core.terminal.rows() {
            let mut row = String::new();
            for x in 0..self.core.terminal.cols() {
                let cell = self.core.terminal.view_cell(x, y, 0);
                if cell.width == 0 {
                    continue;
                }
                row.extend(self.core.terminal.cell_chars(cell));
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
            Value::from(String::from_utf8_lossy(&chunk.bytes).into_owned()),
        );
        value.set("output_base64", Value::from(chunk.output_base64()));
        value.set("cursor", Value::from(chunk.next_cursor));
        value.set("next_cursor", Value::from(chunk.next_cursor));
        value.set("first_cursor", Value::from(chunk.first_cursor));
        value.set("truncated", Value::Bool(chunk.truncated));
        value.set("has_more", Value::Bool(chunk.has_more));
        value.set("screen", Value::from(screen));
        value.set("screen_truncated", Value::Bool(clipped));
        if let Some(timed_out) = timed_out {
            value.set("timed_out", Value::Bool(timed_out));
        }
        value
    }

    /// Waiting for a terminal change neither copies the output ring nor walks
    /// the screen. SSH viewers fetch one frame only after this condition wakes.
    fn frame_changed(&self, revision: u64, control_epoch: u64) -> bool {
        self.core.terminal.text_revision() != revision
            || self.core.control_epoch() != control_epoch
            || (self.core.eof() && self.status().is_some())
    }

    fn frame_revision(&self) -> Value {
        json::object([
            ("revision", self.core.terminal.text_revision().into()),
            ("control_epoch", self.core.control_epoch().into()),
            ("output_eof", self.core.eof().into()),
        ])
    }

    pub(super) fn watch_frame(&self, request: &Value) -> Result<Reply, String> {
        let revision = request
            .get("revision")
            .and_then(Value::as_u64)
            .ok_or("revision is required")?;
        let epoch = request
            .get("control_epoch")
            .and_then(Value::as_u64)
            .ok_or("control_epoch is required")?;
        let wait_ms = bounded(request, "wait_ms", 30000, 0, 30000)? as u64;
        if !self.frame_changed(revision, epoch) && wait_ms > 0 {
            return Ok(Reply::ReadWait(ReadWait {
                session_id: self.id,
                until: None,
                cursor: 0,
                max_bytes: None,
                deadline: os::milliseconds() + wait_ms,
                watch_revision: Some((revision, epoch)),
                input_drain: None,
            }));
        }
        Ok(Reply::Value(self.frame_revision()))
    }

    pub(super) fn terminal_frame(&self, request: &Value) -> Result<Value, String> {
        let history_since = request
            .get("history_since")
            .map(|value| value.as_u64().ok_or("history_since must be an integer"))
            .transpose()?;
        let cols = bounded(request, "view_cols", self.core.terminal.cols(), 2, 512)?;
        let rows = bounded(request, "view_rows", self.core.terminal.rows(), 2, 256)?;
        let mut frame =
            ztty::terminal_frame::capture_view(&self.core.terminal, history_since, cols, rows)?;
        frame.set("revision", self.core.terminal.text_revision().into());
        frame.set("control_epoch", self.core.control_epoch().into());
        frame.set("session", self.info());
        Ok(frame)
    }
}
