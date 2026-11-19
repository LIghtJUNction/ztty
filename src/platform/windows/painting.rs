//! Terminal scene rendering and native paint dispatch.

use super::*;

impl App {
    pub(super) fn images(&mut self, index: usize, range: core::ops::Range<i64>) {
        for draw in self.panes[index]
            .core
            .terminal
            .graphics_draws(self.panes[index].history)
        {
            if range.contains(&i64::from(draw.z)) {
                self.surface.image(
                    &draw,
                    self.panes[index].rect,
                    self.appearance.padding,
                    (
                        self.panes[index].core.terminal.cols(),
                        self.panes[index].core.terminal.rows(),
                    ),
                    (self.fonts.width, self.fonts.height),
                );
            }
        }
    }
    pub(super) fn draw_pane(&mut self, index: usize) {
        if self.surface.pixels.is_null() {
            return;
        }
        // GDI batches drawing. Flush before touching its bitmap from Rust.
        unsafe {
            ffi::GdiFlush();
        }
        let focused = self.focused && index == self.active;
        let rect = self.panes[index].rect;
        self.surface.clip = rect;
        let px = rect.x + self.appearance.padding;
        let py = rect.y + self.appearance.padding;
        let saved = unsafe { ffi::SaveDC(self.surface.dc) };
        if saved == 0 {
            return;
        }
        if unsafe {
            ffi::IntersectClipRect(
                self.surface.dc,
                rect.x,
                rect.y,
                rect.x + rect.width,
                rect.y + rect.height,
            )
        } == 0
        {
            unsafe {
                ffi::RestoreDC(self.surface.dc, saved);
            }
            return;
        }
        let cw = self.fonts.width;
        let ch = self.fonts.height;
        self.surface.fill(
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            self.appearance.theme.background,
        );
        for row in 0..self.panes[index].core.terminal.rows() {
            for col in 0..self.panes[index].core.terminal.cols() {
                let cell =
                    self.panes[index]
                        .core
                        .terminal
                        .view_cell(col, row, self.panes[index].history);
                let bg = if cell.flags & zterm::INVERSE != 0 {
                    cell.fg
                } else {
                    cell.bg
                };
                self.surface
                    .fill(px + col as i32 * cw, py + row as i32 * ch, cw, ch, bg);
            }
        }
        self.images(index, i32::MIN as i64..-1073741824);
        for row in 0..self.panes[index].core.terminal.rows() {
            for col in 0..self.panes[index].core.terminal.cols() {
                let cell =
                    self.panes[index]
                        .core
                        .terminal
                        .view_cell(col, row, self.panes[index].history);
                if cell.flags & (zterm::EXPLICIT_BG | zterm::INVERSE) != 0 {
                    self.surface.fill(
                        px + col as i32 * cw,
                        py + row as i32 * ch,
                        cw,
                        ch,
                        if cell.flags & zterm::INVERSE != 0 {
                            cell.fg
                        } else {
                            cell.bg
                        },
                    );
                }
            }
        }
        self.images(index, -1073741824..0);
        for row in 0..self.panes[index].core.terminal.rows() {
            for col in 0..self.panes[index].core.terminal.cols() {
                if self.panes[index]
                    .selection
                    .is_some_and(|s| s.contains(col, row, self.panes[index].core.terminal.cols()))
                {
                    self.surface.fill(
                        px + col as i32 * cw,
                        py + row as i32 * ch,
                        cw,
                        ch,
                        self.appearance.selection_background,
                    );
                }
            }
        }
        unsafe {
            ffi::SetBkMode(self.surface.dc, 1);
        }
        let previous_font = unsafe { ffi::SelectObject(self.surface.dc, self.fonts.normal) };
        for row in 0..self.panes[index].core.terminal.rows() {
            for col in 0..self.panes[index].core.terminal.cols() {
                let cell =
                    self.panes[index]
                        .core
                        .terminal
                        .view_cell(col, row, self.panes[index].history);
                if cell.width == 0 || cell.flags & zterm::HIDDEN != 0 || cell.ch == '\u{10eeee}' {
                    continue;
                }
                let mut fg = if cell.flags & zterm::INVERSE != 0 {
                    cell.bg
                } else {
                    cell.fg
                };
                if cell.flags & zterm::DIM != 0 {
                    fg = ((fg & 0xfefefe) >> 1) & 0x7f7f7f;
                }
                if self.panes[index]
                    .selection
                    .is_some_and(|s| s.contains(col, row, self.panes[index].core.terminal.cols()))
                {
                    fg = self.appearance.selection_foreground;
                }
                let x = px + col as i32 * cw;
                let y = py + row as i32 * ch;
                let clip = ffi::Rect {
                    left: x,
                    top: y,
                    right: (x + cw * cell.width as i32)
                        .min(px + self.panes[index].core.terminal.cols() as i32 * cw),
                    bottom: y + ch,
                };
                let mut text = [0u16; zterm::MAX_GRAPHEME_BYTES];
                let mut length = 0;
                for scalar in self.panes[index].core.terminal.cell_chars(cell) {
                    let mut units = [0u16; 2];
                    let units = scalar.encode_utf16(&mut units);
                    text[length..length + units.len()].copy_from_slice(units);
                    length += units.len();
                }
                unsafe {
                    ffi::SelectObject(self.surface.dc, self.fonts.face(cell.flags));
                    ffi::SetTextColor(self.surface.dc, colorref(fg));
                    ffi::ExtTextOutW(
                        self.surface.dc,
                        x,
                        y,
                        4,
                        &clip,
                        text.as_ptr(),
                        length as u32,
                        ptr::null(),
                    );
                }
            }
        }
        unsafe {
            ffi::SelectObject(self.surface.dc, previous_font);
            ffi::GdiFlush();
        }
        for row in 0..self.panes[index].core.terminal.rows() {
            for col in 0..self.panes[index].core.terminal.cols() {
                let cell =
                    self.panes[index]
                        .core
                        .terminal
                        .view_cell(col, row, self.panes[index].history);
                if cell.width == 0 || cell.flags & zterm::HIDDEN != 0 || cell.ch == '\u{10eeee}' {
                    continue;
                }
                let fg = if cell.flags & zterm::INVERSE != 0 {
                    cell.bg
                } else {
                    cell.fg
                };
                let x = px + col as i32 * cw;
                let y = py + row as i32 * ch;
                if cell.flags & zterm::UNDERLINE != 0 {
                    self.surface
                        .fill(x, y + ch - 2, cw * cell.width as i32, 1, fg);
                }
                if cell.flags & zterm::STRIKE != 0 {
                    self.surface
                        .fill(x, y + ch / 2, cw * cell.width as i32, 1, fg);
                }
            }
        }
        self.images(index, 0..i32::MAX as i64 + 1);
        if self.panes[index].history == 0
            && self.panes[index].core.terminal.modes().cursor_visible
            && (self.panes[index].cursor_on
                || !focused
                || !self.panes[index]
                    .core
                    .terminal
                    .modes()
                    .cursor_blink
                    .unwrap_or(self.appearance.cursor_blink))
        {
            let (col, row) = self.panes[index].core.terminal.cursor();
            let x = px + col as i32 * cw;
            let y = py + row as i32 * ch;
            let color = self.appearance.cursor_color;
            match self.panes[index]
                .core
                .terminal
                .modes()
                .cursor_shape
                .unwrap_or(self.appearance.cursor_shape)
            {
                CursorShape::Underline => self.surface.fill(x, y + ch - 2, cw, 2, color),
                CursorShape::Bar => self.surface.fill(x, y, 2, ch, color),
                CursorShape::Block => {
                    if !focused {
                        self.surface.fill(x, y, cw, 1, color);
                        self.surface.fill(x, y + ch - 1, cw, 1, color);
                        self.surface.fill(x, y, 1, ch, color);
                        self.surface.fill(x + cw - 1, y, 1, ch, color);
                    } else {
                        let cell = self.panes[index].core.terminal.cell(col, row);
                        let width = cw * i32::from(cell.width.max(1));
                        self.surface.fill(x, y, width, ch, color);
                        if cell.width != 0
                            && cell.flags & zterm::HIDDEN == 0
                            && cell.ch != '\u{10eeee}'
                        {
                            let mut text = [0u16; zterm::MAX_GRAPHEME_BYTES];
                            let mut length = 0;
                            for scalar in self.panes[index].core.terminal.cell_chars(cell) {
                                let mut units = [0u16; 2];
                                let units = scalar.encode_utf16(&mut units);
                                text[length..length + units.len()].copy_from_slice(units);
                                length += units.len();
                            }
                            let mut foreground = if cell.flags & zterm::INVERSE != 0 {
                                cell.fg
                            } else {
                                cell.bg
                            };
                            if foreground == color {
                                foreground = !color & 0xffffff;
                            }
                            let clip = ffi::Rect {
                                left: x,
                                top: y,
                                right: (x + width)
                                    .min(px + self.panes[index].core.terminal.cols() as i32 * cw),
                                bottom: y + ch,
                            };
                            unsafe {
                                let previous =
                                    ffi::SelectObject(self.surface.dc, self.fonts.face(cell.flags));
                                ffi::SetTextColor(self.surface.dc, colorref(foreground));
                                ffi::ExtTextOutW(
                                    self.surface.dc,
                                    x,
                                    y,
                                    4,
                                    &clip,
                                    text.as_ptr(),
                                    length as u32,
                                    ptr::null(),
                                );
                                ffi::SelectObject(self.surface.dc, previous);
                                ffi::GdiFlush();
                            }
                        }
                    }
                }
            }
        }
        unsafe {
            ffi::RestoreDC(self.surface.dc, saved);
        }
        self.panes[index].dirty = false;
        self.panes[index].core.terminal.clear_dirty();
    }
    pub(super) fn paint(&mut self, events: &NativeEvents) {
        let start_event_sequence = events.next();
        let (default_message, default_depth) = if events.enabled {
            let context = events.record.get();
            (context.default_message, context.default_depth)
        } else {
            (0, 0)
        };
        if self.full_redraw {
            unsafe {
                ffi::GdiFlush();
            }
            self.surface.clip = Rect {
                x: 0,
                y: 0,
                width: self.surface.width,
                height: self.surface.height,
            };
            self.surface.fill(
                0,
                0,
                self.surface.width,
                self.surface.height,
                self.appearance.theme.background,
            );
        }
        for index in 0..self.panes.len() {
            let pane = &self.panes[index];
            if pane.visible && pane.core.terminal.modes().synchronized_output {
                if self.full_redraw {
                    let rect = pane.rect;
                    self.surface.clip = rect;
                    self.surface.fill(
                        rect.x,
                        rect.y,
                        rect.width,
                        rect.height,
                        self.appearance.theme.background,
                    );
                    if let Some(frame) = &pane.held_frame {
                        self.surface.restore_snapshot(frame, rect);
                    }
                }
                continue;
            }
            if pane.visible
                && !pane.core.terminal.modes().synchronized_output
                && (self.full_redraw || pane.dirty || pane.core.terminal.dirty())
            {
                self.draw_pane(index);
            }
        }
        self.full_redraw = false;
        // Prepare the retained frame before acquiring the window DC. Keep its
        // lifetime limited to presentation and the existing opt-in diagnostics.
        let mut paint: ffi::PaintStruct = unsafe { core::mem::zeroed() };
        let dc = unsafe { ffi::BeginPaint(self.hwnd, &mut paint) };
        let diagnostic_sample = (events.paint_diagnostics
            && !dc.is_null()
            && self.surface.width > 0
            && self.surface.height > 0)
            .then(|| {
                // Keep an explicitly configured point unchanged across resize,
                // including when it falls outside the temporarily smaller DIB.
                events.paint_sample.get().unwrap_or([
                    self.surface.width.saturating_sub(2).max(0),
                    self.surface.height / 2,
                ])
            });
        let cpu_before_blit = diagnostic_sample.and_then(|[x, y]| self.diagnostic_cpu_pixel(x, y));
        let mut blit = 0;
        if !self.surface.bitmap.is_null() {
            unsafe {
                blit = ffi::BitBlt(
                    dc,
                    0,
                    0,
                    self.surface.width,
                    self.surface.height,
                    self.surface.dc,
                    0,
                    0,
                    0x00cc0020,
                );
            }
        }
        // Opt-in only: flush immediately after BitBlt, then sample target before
        // source or clip queries can hide a failed batch. Never an acceptance flush.
        let target = if let Some([x, y]) = diagnostic_sample {
            let flush_after_blit = unsafe { ffi::GdiFlush() };
            let target_pixel = unsafe { ffi::GetPixel(dc, x, y) };
            let source_pixel = unsafe { ffi::GetPixel(self.surface.dc, x, y) };
            let mut clip = ffi::Rect::default();
            let mut origin = ffi::Point::default();
            let clip_type = unsafe { ffi::GetClipBox(dc, &mut clip) };
            let origin_ok = unsafe { ffi::GetDCOrgEx(dc, &mut origin) } != 0;
            Some(PaintTarget {
                sample: [x, y],
                cpu_before_blit,
                flush_after_blit,
                target_after_flush: (target_pixel != u32::MAX).then(|| colorref(target_pixel)),
                source_after_target_read: (source_pixel != u32::MAX)
                    .then(|| colorref(source_pixel)),
                clip_type,
                clip: [clip.left, clip.top, clip.right, clip.bottom],
                origin_ok,
                origin: [origin.x, origin.y],
            })
        } else {
            None
        };
        self.trace_frame("paint", dc, &paint.paint, blit);
        let end_paint_return = unsafe { ffi::EndPaint(self.hwnd, &paint) };
        let end_event_sequence = events.next();
        // Opt-in only: compare the same pixel through a fresh client DC after
        // EndPaint. Keep both samples local until the complete paint is recorded.
        let target_after_end = diagnostic_sample.map(|sample| {
            let client_dc = unsafe { ffi::GetDC(self.hwnd) };
            let (target_rgb, release_dc_return) = if client_dc.is_null() {
                (None, None)
            } else {
                let pixel = unsafe { ffi::GetPixel(client_dc, sample[0], sample[1]) };
                let released = unsafe { ffi::ReleaseDC(self.hwnd, client_dc) };
                ((pixel != u32::MAX).then(|| colorref(pixel)), Some(released))
            };
            PaintAfterEnd {
                sample,
                sample_end_event_sequence: events.next(),
                end_paint_return,
                client_dc_ok: !client_dc.is_null(),
                target_rgb,
                release_dc_return,
            }
        });
        if self.native_snapshot_path.is_some() {
            self.last_paint = PaintRecord {
                sequence: self.last_paint.sequence.saturating_add(1),
                surface: [self.surface.width, self.surface.height],
                region: [
                    paint.paint.left,
                    paint.paint.top,
                    paint.paint.right,
                    paint.paint.bottom,
                ],
                blit,
                start_event_sequence,
                end_event_sequence,
                default_message,
                default_depth,
                target,
                target_after_end,
            };
        }
    }
}
