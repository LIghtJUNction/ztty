//! Pane construction, output pumping and wake deadlines.

use super::*;

impl Pane {
    pub(super) fn new(
        id: u64,
        pty: zpty::Pty,
        terminal: Terminal,
        size: zpty::Size,
        rect: Rect,
        wake: &Arc<session_wait::Wake>,
    ) -> Result<Self, &'static str> {
        let output_wait = session_wait::Wait::new(pty.output_event(), wake)?;
        let process_wait = session_wait::Wait::new(pty.process_event(), wake)?;
        Ok(Self {
            output_wait,
            _process_wait: process_wait,
            id,
            visible: true,
            pty_size: size,
            title: String::new(),
            rect,
            core: SessionCore::new(pty, terminal, String::from("local"), MAX_INPUT),
            dirty: true,
            history: 0,
            selection: None,
            dragging: false,
            graphics_revision: 0,
            sync_since: None,
            held_frame: None,
            cursor_on: true,
            blink_deadline: os::milliseconds() + 500,
            last_cursor: (0, 0),
        })
    }

    pub(super) fn grid_ready(&self) -> bool {
        self.core.terminal.cols() == usize::from(self.pty_size.cols)
            && self.core.terminal.rows() == usize::from(self.pty_size.rows)
    }
}

impl App {
    pub(super) fn message(&mut self, message: &str) {
        os::print(2, b"ztty: ");
        os::print(2, message.as_bytes());
        os::print(2, b"\n");
        self.notice = Some((String::from(message), os::milliseconds() + 3000));
        self.caption_dirty = true;
    }

    pub(super) fn caption(&mut self, now: u64) {
        if self.notice.as_ref().is_some_and(|(_, until)| now >= *until) {
            self.notice = None;
            self.caption_dirty = true;
        }
        let pane = &mut self.panes[self.active];
        if pane.title != pane.core.terminal.title() {
            pane.title = String::from(pane.core.terminal.title());
            self.caption_dirty = true;
        }
        if !self.caption_dirty {
            return;
        }
        let caption = if let Some((message, _)) = &self.notice {
            format!("ztty — {message}")
        } else {
            let title = if pane.title.is_empty() {
                "ztty"
            } else {
                &pane.title
            };
            if self.layout.tabs.len() > 1 {
                format!(
                    "[{} / {}] {title}",
                    self.layout.active_tab + 1,
                    self.layout.tabs.len()
                )
            } else {
                String::from(title)
            }
        };
        if caption != self.title {
            self.title = caption;
            let title = wide(&self.title);
            unsafe {
                ffi::SetWindowTextW(self.hwnd, title.as_ptr());
            }
        }
        self.caption_dirty = false;
    }
    pub(super) fn tick(&mut self) {
        if self.panes.is_empty() {
            self.running = false;
            return;
        }
        let now = os::milliseconds();
        if self.resize_pending && now >= self.resize_retry_at {
            let _ = self.arrange(self.layout.clone());
        }
        let mut buffer = [0u8; 65536];
        let count = self.panes.len();
        // At most 1 MiB per tick, rotating the first pane so a busy hidden tab
        // cannot starve foreground input or another PTY's responses.
        let mut data = 0;
        let first = self.pump_cursor;
        for offset in 0..count {
            let index = (first + offset) % count;
            let pane = &mut self.panes[index];
            let _ = pane.core.flush_input();
            if pane.grid_ready() {
                for _ in 0..(16 / count).max(1) {
                    match pane.core.read_once(&mut buffer, false) {
                        Ok(ReadStep::Data {
                            history_scrolled,
                            history_after,
                            ..
                        }) => {
                            if pane.history != 0 {
                                pane.history = pane
                                    .history
                                    .saturating_add(history_scrolled)
                                    .min(history_after);
                            }
                            pane.dirty = true;
                            data += 1;
                        }
                        Ok(ReadStep::Pending | ReadStep::Eof) | Err(_) => break,
                    }
                    if data >= 16 {
                        break;
                    }
                }
            }
            if !pane.core.queue_responses() {
                os::print(
                    2,
                    b"ztty: input queue full; terminal replies were rejected\n",
                );
            }
            let _ = pane.core.flush_input();
            let _ = pane.core.pty.reap();
            if pane.grid_ready() {
                pane.output_wait.rearm();
            }
            self.pump_cursor = (index + 1) % count;
            if data >= 16 {
                break;
            }
        }
        self.pump_again = data >= 16;
        for index in (0..self.panes.len()).rev() {
            if self.panes[index].core.eof() || self.panes[index].core.pty.status().is_some() {
                self.close_pane(index);
            }
        }
        if self.panes.is_empty() {
            return;
        }
        let mut position_ime = false;
        for (index, pane) in self.panes.iter_mut().enumerate() {
            let cursor = pane.core.terminal.cursor();
            if cursor != pane.last_cursor {
                pane.last_cursor = cursor;
                pane.cursor_on = true;
                pane.blink_deadline = now + self.appearance.cursor_blink_interval;
                pane.dirty = true;
                position_ime |= index == self.active;
            }
            if pane.core.terminal.modes().synchronized_output {
                let start = *pane.sync_since.get_or_insert(now);
                if now.saturating_sub(start) >= 150 {
                    pane.core.terminal.force_end_sync();
                    pane.sync_since = None;
                }
            } else {
                pane.sync_since = None;
            }
            if !pane.core.terminal.modes().synchronized_output {
                pane.held_frame = None;
            }
            if pane.visible
                && index == self.active
                && self.focused
                && pane
                    .core
                    .terminal
                    .modes()
                    .cursor_blink
                    .unwrap_or(self.appearance.cursor_blink)
                && now >= pane.blink_deadline
            {
                pane.cursor_on = !pane.cursor_on;
                pane.blink_deadline = now + self.appearance.cursor_blink_interval;
                pane.dirty = true;
            }
            let images_visible = pane.visible && !pane.core.eof();
            osc52::process(self.hwnd, &mut pane.core, images_visible);
            pane.core
                .terminal
                .advance_graphics(now, pane.history, images_visible);
            let revision = pane.core.terminal.graphics().revision();
            if revision != pane.graphics_revision {
                pane.graphics_revision = revision;
                pane.dirty = true;
            }
        }
        if position_ime {
            self.ime_position();
        }
        self.caption(now);
        if self.full_redraw
            || self.panes.iter().any(|pane| {
                pane.visible
                    && !pane.core.terminal.modes().synchronized_output
                    && (pane.dirty || pane.core.terminal.dirty())
            })
        {
            unsafe {
                ffi::InvalidateRect(self.hwnd, ptr::null(), 0);
            }
        }
    }

    pub(super) fn wait_timeout(&self) -> u32 {
        if self.pump_again {
            return 0;
        }
        let now = os::milliseconds();
        let mut timeout = 1000;
        for (index, pane) in self.panes.iter().enumerate() {
            if pane.visible
                && index == self.active
                && self.focused
                && pane
                    .core
                    .terminal
                    .modes()
                    .cursor_blink
                    .unwrap_or(self.appearance.cursor_blink)
            {
                timeout = timeout.min(pane.blink_deadline.saturating_sub(now).min(1000) as u32);
            }
            if let Some(deadline) = pane.core.terminal.graphics().animation_deadline() {
                timeout = timeout.min(deadline.saturating_sub(now).min(1000) as u32);
            }
            if pane.core.input.pending() {
                timeout = timeout.min(8);
            }
            if let Some(start) = pane.sync_since {
                timeout = timeout.min((start + 150).saturating_sub(now).min(1000) as u32);
            }
        }
        if self.resize_pending {
            timeout = timeout.min(self.resize_retry_at.saturating_sub(now).min(1000) as u32);
        }
        if let Some((_, until)) = self.notice.as_ref() {
            timeout = timeout.min(until.saturating_sub(now).min(1000) as u32);
        }
        timeout
    }
}
