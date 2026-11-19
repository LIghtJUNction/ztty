//! Pane layout and coordinated font and PTY resizing.

use super::*;

impl App {
    pub(super) fn resize(&mut self) {
        self.resize_with_fonts(None);
    }
    pub(super) fn resize_with_fonts(&mut self, candidate: Option<Fonts>) {
        if let Some(fonts) = candidate {
            self.pending_fonts = Some(fonts);
        }
        if let Err(error) = self.arrange(self.layout.clone()) {
            self.message(error);
        }
    }

    pub(super) fn font_action(&mut self, action: &str) -> Result<(), &'static str> {
        // Repeated shortcuts build on the requested font even while PTY
        // compensation is pending; arrange commits font and all grids together.
        let current = self.pending_fonts.as_ref().unwrap_or(&self.fonts);
        let description = if action == "font-reset" {
            self.appearance.font.clone()
        } else {
            let size = if action == "font-increase" {
                current.size.saturating_add(1)
            } else {
                current.size.saturating_sub(1)
            }
            .clamp(5, 96);
            if size == current.size {
                return Ok(());
            }
            let mut description = String::new();
            for (index, part) in current.description.split(':').enumerate() {
                if index != 0 && part.starts_with("size=") {
                    continue;
                }
                if index != 0 {
                    description.push(':');
                }
                description.push_str(part);
            }
            format!("{description}:size={size}")
        };
        let candidate = Fonts::new(self.surface.dc, &description, unsafe {
            ffi::GetDpiForWindow(self.hwnd)
        })?;
        self.resize_with_fonts(Some(candidate));
        Ok(())
    }

    pub(super) fn grid_size(cols: usize, rows: usize, fonts: &Fonts) -> zpty::Size {
        zpty::Size {
            cols: cols as u16,
            rows: rows as u16,
            pixel_width: (cols as i32 * fonts.width).min(u16::MAX as i32) as u16,
            pixel_height: (rows as i32 * fonts.height).min(u16::MAX as i32) as u16,
        }
    }

    pub(super) fn size_in(rect: Rect, fonts: &Fonts, padding: i32) -> zpty::Size {
        let cols =
            ((rect.width - 2 * padding).max(fonts.width) / fonts.width).clamp(1, 1024) as usize;
        let rows =
            ((rect.height - 2 * padding).max(fonts.height) / fonts.height).clamp(1, 1024) as usize;
        Self::grid_size(cols, rows, fonts)
    }

    pub(super) fn preserve_held_frames(&mut self) -> Result<(), &'static str> {
        let now = os::milliseconds();
        for pane in &mut self.panes {
            if pane.core.terminal.modes().synchronized_output {
                let start = *pane.sync_since.get_or_insert(now);
                if now.saturating_sub(start) >= 150 {
                    pane.core.terminal.force_end_sync();
                    pane.sync_since = None;
                }
            }
            if !pane.core.terminal.modes().synchronized_output {
                pane.held_frame = None;
            }
        }
        let used: usize = self
            .panes
            .iter()
            .filter_map(|pane| pane.held_frame.as_ref())
            .map(session_frame::Snapshot::pixel_count)
            .sum();
        let mut remaining = MAX_SURFACE_PIXELS.saturating_sub(used);
        for pane in &mut self.panes {
            if pane.visible
                && pane.core.terminal.modes().synchronized_output
                && pane.held_frame.is_none()
            {
                let frame = self.surface.snapshot(pane.rect, remaining)?;
                remaining -= frame.pixel_count();
                pane.held_frame = Some(frame);
            }
        }
        Ok(())
    }

    /// Coordinate all live PTYs before committing any terminal grid/view/font.
    /// Failed compensation is tracked by pty_size; that pane's output is held
    /// in zpty's bounded queue until a retry restores a matching parser grid.
    pub(super) fn arrange(&mut self, layout: Layout) -> Result<(), &'static str> {
        self.resize_pending = true;
        self.resize_retry_at = os::milliseconds() + 250;
        let mut client = ffi::Rect::default();
        if unsafe { ffi::GetClientRect(self.hwnd, &mut client) } == 0 {
            return Err("cannot query the Windows workspace dimensions");
        }
        let width = client.right.max(1);
        let height = client.bottom.max(1);
        self.trace_frame("arrange-start", NULL, &ffi::Rect::default(), 0);
        let changed = self.surface.width != width || self.surface.height != height;
        let placements = layout.placements(width, height);
        let geometry_changed = changed
            || self.pending_fonts.is_some()
            || self.panes.iter().any(|pane| {
                let next = placements.iter().find(|(id, _)| *id == pane.id);
                pane.visible != next.is_some() || next.is_some_and(|(_, rect)| pane.rect != *rect)
            });
        if geometry_changed || self.full_redraw {
            // Do this before replacing the DIB or changing pane coordinates.
            // Repeated resizes keep the original committed snapshot.
            self.preserve_held_frames()?;
        }
        let previous_surface = [self.surface.width, self.surface.height];
        if !self.surface.resize(width, height) {
            return Err("cannot allocate the Windows workspace surface");
        }
        if changed && self.native_snapshot_path.is_some() {
            // Include the first DIB allocation, but not no-ops or failed sizes.
            // Later PTY coordination can still reject this layout.
            self.surface_resize = Some(SurfaceResizeRecord {
                count: self
                    .surface_resize
                    .as_ref()
                    .map_or(1, |resize| resize.count.saturating_add(1)),
                from: previous_surface,
                to: [self.surface.width, self.surface.height],
            });
        }
        self.full_redraw |= changed;
        let fonts = self.pending_fonts.as_ref().unwrap_or(&self.fonts);
        let mut plan = Vec::with_capacity(self.panes.len());
        for pane in &self.panes {
            let rect = placements
                .iter()
                .find(|(id, _)| *id == pane.id)
                .map(|(_, rect)| *rect);
            let size = rect.map_or_else(
                || Self::grid_size(pane.core.terminal.cols(), pane.core.terminal.rows(), fonts),
                |rect| Self::size_in(rect, fonts, self.appearance.padding),
            );
            let rollback = Self::grid_size(
                pane.core.terminal.cols(),
                pane.core.terminal.rows(),
                &self.fonts,
            );
            plan.push((rect, size, rollback));
        }
        let mut sizes: Vec<session_resize::Plan> = self
            .panes
            .iter()
            .zip(&plan)
            .map(|(pane, (_, desired, committed))| session_resize::Plan {
                actual: pane.pty_size,
                desired: *desired,
                committed: *committed,
            })
            .collect();
        let result =
            session_resize::coordinate(&mut sizes, self.pending_fonts.is_some(), |index, size| {
                self.panes[index].core.eof() || self.panes[index].core.pty.resize(size).is_ok()
            });
        for (pane, size) in self.panes.iter_mut().zip(sizes) {
            pane.pty_size = size.actual;
        }
        if let Err(error) = result {
            return Err(if error.rollback_failed {
                "workspace PTY rollback incomplete; affected output paused for retry"
            } else {
                "workspace PTY resize failed; previous layout retained and retry scheduled"
            });
        }
        let old_focus = self.panes.get(self.active).map(|pane| pane.id);
        if let Some(fonts) = self.pending_fonts.take() {
            self.fonts = fonts;
        }
        for (pane, (rect, size, _)) in self.panes.iter_mut().zip(plan) {
            let grid_changed = pane.core.terminal.cols() != usize::from(size.cols)
                || pane.core.terminal.rows() != usize::from(size.rows);
            pane.core
                .terminal
                .set_cell_size(self.fonts.width as u32, self.fonts.height as u32);
            if grid_changed {
                let mut anchor = ztty::selection::ViewAnchor::capture(
                    &pane.core.terminal,
                    pane.history,
                    pane.selection
                        .map(|selection| (selection.start, selection.end)),
                );
                pane.core.terminal.resize_anchored(
                    usize::from(size.cols),
                    usize::from(size.rows),
                    anchor.anchors_mut(),
                );
                let restored = anchor.restore(&pane.core.terminal);
                pane.history = restored.history;
                pane.selection = restored
                    .selection
                    .map(|(start, end)| Selection { start, end });
                pane.dragging = false;
            }
            pane.pty_size = size;
            pane.history = pane.history.min(pane.core.terminal.history_len());
            pane.visible = rect.is_some();
            if let Some(rect) = rect {
                pane.rect = rect;
            }
            pane.dirty = true;
        }
        self.layout = layout;
        self.active = self
            .panes
            .iter()
            .position(|pane| pane.id == self.layout.focused)
            .unwrap_or(0);
        self.resize_pending = false;
        self.full_redraw |= geometry_changed;
        self.caption_dirty = true;
        if old_focus != self.panes.get(self.active).map(|pane| pane.id) {
            if let Some(old) =
                old_focus.and_then(|id| self.panes.iter().position(|pane| pane.id == id))
            {
                let bytes =
                    input::encode_focus(false, &self.panes[old].core.terminal.modes().input());
                self.panes[old].core.input.push(&bytes);
            }
            for (held, intercepted) in self.held.iter_mut().zip(&mut self.intercepted) {
                if held.take().is_some() {
                    *intercepted = true;
                }
            }
            self.pending_key = None;
            self.surrogate = 0;
            self.suppress_char = true;
            self.cancel_mouse_capture(true);
            let bytes = input::encode_focus(
                self.focused,
                &self.panes[self.active].core.terminal.modes().input(),
            );
            self.panes[self.active].core.input.push(&bytes);
            self.reset_cursor();
        }
        // Origin, cell metrics or viewport can change without cursor movement.
        self.ime_position();
        unsafe {
            ffi::InvalidateRect(self.hwnd, ptr::null(), 0);
        }
        self.trace_frame("arrange-commit", NULL, &ffi::Rect::default(), 0);
        Ok(())
    }

    pub(super) fn related(&mut self, axis: Option<Axis>) -> Result<(), &'static str> {
        let id = self.next_id;
        let next = id.checked_add(1).ok_or("workspace session IDs exhausted")?;
        let mut layout = self.layout.clone();
        if let Some(axis) = axis {
            layout.split(id, axis)?;
        } else {
            layout.add_tab(id)?;
        }
        let placements = layout.placements(self.surface.width, self.surface.height);
        let rect = placements
            .iter()
            .find(|(pane, _)| *pane == id)
            .map(|(_, rect)| *rect)
            .ok_or("new pane has no placement")?;
        let size = Self::size_in(rect, &self.fonts, self.appearance.padding);
        let program = os::resolve(&self.launch.shell)?;
        let arguments: Vec<u16> = self.launch.shell.to_string_lossy().encode_utf16().collect();
        let pty = zpty::Pty::spawn_utf16(
            &program,
            &[&arguments],
            Some(&self.launch.environment),
            size,
        )
        .map_err(|_| "cannot start the new Windows pseudoconsole")?;
        let mut terminal = Terminal::with_theme(
            usize::from(size.cols),
            usize::from(size.rows),
            self.launch.history,
            self.appearance.theme,
        );
        terminal.set_cell_size(self.fonts.width as u32, self.fonts.height as u32);
        let mut pane = Pane::new(id, pty, terminal, size, rect, &self.wake)?;
        pane.visible = false;
        self.panes.push(pane);
        if let Err(error) = self.arrange(layout) {
            // Only this newly created PTY is discarded. Existing identities stay.
            self.panes.pop();
            return Err(error);
        }
        self.next_id = next;
        Ok(())
    }

    pub(super) fn close_pane(&mut self, index: usize) {
        if index >= self.panes.len() {
            return;
        }
        let old_focus = self.panes.get(self.active).map(|pane| pane.id);
        let id = self.panes[index].id;
        self.last_status = self.panes[index]
            .core
            .pty
            .status()
            .and_then(|status| status.code())
            .unwrap_or(0);
        self.layout.remove(id);
        let pane = self.panes.remove(index);
        if pane.dragging || self.mouse_capture.is_some_and(|(owner, _)| owner == id) {
            self.cancelled_mouse_buttons |= u8::from(pane.dragging);
            self.cancel_mouse_capture(true);
        }
        drop(pane); // Waits cancel/join before its PTY closes its own process tree.
        if self.panes.is_empty() {
            self.active = 0;
            self.running = false;
            return;
        }
        self.active = self
            .panes
            .iter()
            .position(|pane| pane.id == self.layout.focused)
            .unwrap_or(0);
        let focus_changed = old_focus != Some(self.panes[self.active].id);
        if focus_changed {
            self.pending_key = None;
            self.surrogate = 0;
            self.suppress_char = true;
            for (held, intercepted) in self.held.iter_mut().zip(&mut self.intercepted) {
                if held.take().is_some() {
                    *intercepted = true;
                }
            }
            let bytes = input::encode_focus(
                self.focused,
                &self.panes[self.active].core.terminal.modes().input(),
            );
            self.panes[self.active].core.input.push(&bytes);
        }
        self.resize_with_fonts(None);
        if focus_changed {
            self.reset_cursor();
        }
        self.ime_position();
    }

    pub(super) fn action(&mut self, action: &str) -> bool {
        let result = match action {
            "new-tab" => self.related(None),
            "split-right" => self.related(Some(Axis::Vertical)),
            "split-down" => self.related(Some(Axis::Horizontal)),
            "close" => {
                self.close_pane(self.active);
                Ok(())
            }
            "copy" => {
                self.copy();
                Ok(())
            }
            "paste" => {
                self.paste();
                Ok(())
            }
            "font-increase" | "font-decrease" | "font-reset" => self.font_action(action),
            "scroll-up" | "scroll-down" => {
                let pane = &mut self.panes[self.active];
                let step = pane.core.terminal.rows().saturating_sub(1).max(1);
                pane.history = if action == "scroll-up" {
                    pane.history
                        .saturating_add(step)
                        .min(pane.core.terminal.history_len())
                } else {
                    pane.history.saturating_sub(step)
                };
                pane.selection = None;
                pane.dirty = true;
                Ok(())
            }
            "next-tab" | "previous-tab" | "next-pane" | "previous-pane" | "swap-next"
            | "swap-previous" | "maximize" | "grow" | "shrink" => {
                let mut layout = self.layout.clone();
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
                    _ => {}
                }
                self.arrange(layout)
            }
            "none" => Ok(()),
            _ => return false,
        };
        if let Err(error) = result {
            self.message(error);
        }
        true
    }
}
