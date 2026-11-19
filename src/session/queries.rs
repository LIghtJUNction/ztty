//! Read-only session metadata, output pages and deferred output reads.
use super::{Session, requests::bounded};
use crate::{
    broker::{ReadWait, Reply},
    json::{self, Value},
    os,
};
use alloc::string::String;
use ztty::output_tail::OutputChunk;

impl Session {
    pub(super) fn info(&self) -> Value {
        json::object([
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
        ])
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
        let chunk = self.core.read_output(cursor, max)?;
        if wait_ms > 0 && !self.output_ready(&chunk) {
            return Ok(Reply::ReadWait(ReadWait {
                session_id: self.id,
                cursor,
                max_bytes: max,
                deadline: os::milliseconds() + wait_ms,
            }));
        }
        Ok(Reply::Value(
            self.read_value(chunk, (wait_ms > 0).then_some(false)),
        ))
    }

    pub(super) fn output_ready(&self, chunk: &OutputChunk) -> bool {
        // has_more also includes an incomplete UTF-8 suffix. Keep waiting for
        // its continuation instead of returning an empty page in a busy loop.
        !chunk.bytes.is_empty() || chunk.truncated || self.core.eof()
    }

    pub(super) fn wait_output(&self, wait: &ReadWait) -> Result<Option<Value>, String> {
        let chunk = self.core.read_output(wait.cursor, wait.max_bytes)?;
        let ready = self.output_ready(&chunk);
        if ready || os::milliseconds() >= wait.deadline {
            Ok(Some(self.read_value(chunk, Some(!ready))))
        } else {
            Ok(None)
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
}
