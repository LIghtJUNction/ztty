//! Terminal cell drawing and completed-frame presentation.
use super::*;

impl Window {
    pub fn present(&self) {
        // Expose already requests a full repaint in the view. A swapped EGL
        // back buffer is not retained content and must not be published again.
        if self.gpu.is_none() && !self.gpu_repaint && self.retained_valid {
            self.present_region(0, self.state.height);
        }
    }

    pub(super) fn damage(&mut self, top: i32, bottom: i32) {
        let top = top.clamp(0, self.state.height);
        let bottom = bottom.clamp(top, self.state.height);
        if top == bottom {
            return;
        }
        self.pending_damage = Some(match self.pending_damage {
            Some((start, end)) => (start.min(top), end.max(bottom)),
            None => (top, bottom),
        });
    }

    /// Publish only after terminal rows and view overlays form one complete frame.
    /// Xlib may send its buffer during drawing, so delaying XFlush alone does not
    /// prevent several earlier XCopyArea requests from exposing a partial frame.
    pub fn finish_frame(&mut self) {
        if self.finish_gpu_frame() {
            return;
        }
        if let Some((top, bottom)) = self.pending_damage.take() {
            // render() rebuilds every pixel after allocation or backend changes;
            // publish only after its overlays are complete too.
            self.retained_valid = true;
            self.present_region(top, bottom);
        }
    }

    fn present_region(&self, top: i32, bottom: i32) {
        if self.suspended || self.state.id == 0 || !self.retained_valid {
            return;
        }
        unsafe {
            XCopyArea(
                self.state.display,
                self.pixmap,
                self.state.id,
                self.gc,
                0,
                top,
                self.state.width as u32,
                (bottom - top) as u32,
                0,
                top,
            );
            XFlush(self.state.display);
        }
    }
    fn cell_colors(&self, cell: &Cell, selected: bool) -> (u32, u32, bool) {
        let (mut fg, mut bg) = if cell.flags & INVERSE != 0 {
            (cell.bg, cell.fg)
        } else {
            (cell.fg, cell.bg)
        };
        if cell.flags & DIM != 0 {
            fg = ((fg & 0xfefefe) >> 1) + ((bg & 0xfefefe) >> 1);
        }
        if selected {
            (fg, bg) = self.selection_colors;
        }
        if cell.flags & HIDDEN != 0 {
            fg = bg;
        }
        let translucent = !selected && cell.flags & (INVERSE | EXPLICIT_BG) == 0;
        (fg, bg, translucent)
    }

    fn text_hidden(&self, cell: &Cell) -> bool {
        cell.ch == '\u{10eeee}'
            || cell.flags & HIDDEN != 0
            || (cell.flags & BLINK != 0 && !self.text_blink_visible[0])
            || (cell.flags & RAPID_BLINK != 0 && !self.text_blink_visible[1])
    }
    pub(super) fn fill_background(
        &mut self,
        rgb: u32,
        translucent: bool,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    ) {
        if self.gpu_fill(rgb, x, y, w, h) {
            return;
        }
        if let Some(color) = self.background_fill.as_ref().filter(|_| translucent) {
            // Replace the retained pixels, including alpha, on every damaged row.
            unsafe {
                XRenderFillRectangle(
                    self.state.display,
                    1,
                    XftDrawPicture(self.draw),
                    color,
                    x,
                    y,
                    w,
                    h,
                )
            };
        } else {
            let color = self.color(rgb);
            unsafe { XftDrawRect(self.draw, &color, x, y, w, h) };
        }
    }
    pub fn cursor_changed(&self, cursor: Option<DrawnCursor>) -> bool {
        cursor != self.last_cursor
    }

    pub fn search_matches(&mut self, cols: usize, bits: Vec<u64>) {
        self.search_cols = cols;
        self.search_bits = bits;
    }

    fn search_match(&self, col: usize, row: usize) -> bool {
        let bit = row.saturating_mul(self.search_cols).saturating_add(col);
        self.search_bits
            .get(bit / 64)
            .is_some_and(|word| word & (1 << (bit % 64)) != 0)
    }

    pub fn render(
        &mut self,
        terminal: &mut Terminal,
        offset: usize,
        selection: Option<Selection>,
        force: bool,
        cursor: Option<DrawnCursor>,
    ) {
        if self.render_gpu(terminal, offset, selection, cursor) {
            return;
        }
        // A scene change can remove pixels outside the new placement. Rebuild
        // the visible frame on structural changes; ordinary text still damages
        // only its own rows and re-composites the images intersecting those rows.
        let force =
            force || !self.retained_valid || self.gpu_repaint || self.graphics_changed(terminal);
        self.gpu_repaint = false;
        if self.graphics_alternate != terminal.modes().alternate_screen {
            while !self.images.is_empty() {
                self.discard_upload(self.images.len() - 1);
            }
        }
        if self.graphics_changed(terminal) {
            for index in (0..self.images.len()).rev() {
                let upload = &self.images[index];
                let placed = terminal.graphics_draws(offset).any(|draw| {
                    draw.image_id == upload.image_id && draw.image.generation == upload.generation
                });
                let referenced = terminal.ui_scene().is_some_and(|scene| {
                    scene.nodes.iter().any(|node| {
                        matches!(&node.kind, zterm::ui::Kind::Image { image_id, generation, .. }
                            if *image_id == upload.image_id
                                && generation.is_none_or(|expected| expected == upload.generation))
                    }) && terminal
                        .graphics()
                        .image(upload.image_id)
                        .is_some_and(|image| image.generation == upload.generation)
                });
                if !placed && !referenced {
                    self.discard_upload(index);
                }
            }
        }
        let cols = terminal.cols();
        let rows = terminal.rows();
        let cursor_changed = cursor != self.last_cursor;
        if force {
            self.fill_background(
                self.background,
                true,
                0,
                0,
                self.state.width as u32,
                self.state.height as u32,
            );
        }
        if force {
            self.damage(0, self.state.height);
        }
        for row in 0..rows {
            if !(force
                || terminal.row_dirty(row)
                || (cursor_changed
                    && [cursor, self.last_cursor]
                        .into_iter()
                        .flatten()
                        .any(|((_, y), _)| y == row)))
            {
                continue;
            }
            let y = self.padding + row as i32 * self.state.ch;
            self.damage(y, y + self.state.ch);
            let mut col = 0;
            while col < cols {
                let cell = terminal.view_cell(col, row, offset);
                let selected = selection.is_some_and(|s| s.contains(col, row, cols))
                    || self.search_match(col, row);
                let (_, rgb, translucent) = self.cell_colors(cell, selected);
                let mut end = col + 1;
                while end < cols {
                    let next = terminal.view_cell(end, row, offset);
                    let (_, next_rgb, next_translucent) = self.cell_colors(
                        next,
                        selection.is_some_and(|s| s.contains(end, row, cols))
                            || self.search_match(end, row),
                    );
                    if (next_rgb, next_translucent) != (rgb, translucent) {
                        break;
                    }
                    end += 1;
                }
                self.fill_background(
                    rgb,
                    translucent,
                    self.padding + col as i32 * self.state.cw,
                    y,
                    ((end - col) as i32 * self.state.cw) as u32,
                    self.state.ch as u32,
                );
                col = end;
            }
            self.draw_images(terminal, offset, row, 0);
            // Very negative z is below explicitly colored cells. Reapply only
            // those backgrounds after the bottom image layer.
            if terminal
                .graphics_draws(offset)
                .any(|draw| draw.z < -1_073_741_824)
            {
                for col in 0..cols {
                    let cell = terminal.view_cell(col, row, offset);
                    let selected = selection.is_some_and(|s| s.contains(col, row, cols))
                        || self.search_match(col, row);
                    let (_, rgb, translucent) = self.cell_colors(cell, selected);
                    if !translucent {
                        self.fill_background(
                            rgb,
                            false,
                            self.padding + col as i32 * self.state.cw,
                            y,
                            self.state.cw as u32,
                            self.state.ch as u32,
                        );
                    }
                }
            }
            self.draw_images(terminal, offset, row, 1);
            col = 0;
            while col < cols {
                let cell = terminal.view_cell(col, row, offset);
                if cell.width == 0 || self.text_hidden(cell) {
                    col += 1;
                    continue;
                }
                let (fg, _, _) = self.cell_colors(
                    cell,
                    selection.is_some_and(|s| s.contains(col, row, cols))
                        || self.search_match(col, row),
                );
                let color = self.color(fg);
                let x = self.padding + col as i32 * self.state.cw;
                let baseline = y + unsafe { (*self.font).ascent } + 1;
                let mut text = [0u8; 2048];
                let mut len = 0;
                let mut cells = 1;
                for ch in terminal.cell_chars(cell) {
                    let ch = if ch == '\0' { ' ' } else { ch };
                    len += ch.encode_utf8(&mut text[len..]).len();
                }
                if len == 0 {
                    text[0] = b' ';
                    len = 1;
                }
                let font = if cell.flags & ITALIC != 0 {
                    self.fonts.italic_glyph(cell.ch)
                } else {
                    self.glyph_font(cell.ch)
                };
                // Adjacent simple ASCII cells share a single Xft draw call.
                if cell.ch.is_ascii()
                    && cell.width == 1
                    && cell.combining == ['\0'; 2]
                    && cell.grapheme_id == 0
                {
                    while col + cells < cols && len < text.len() - 4 {
                        let next = terminal.view_cell(col + cells, row, offset);
                        if !next.ch.is_ascii()
                            || next.width != 1
                            || next.combining != ['\0'; 2]
                            || next.grapheme_id != 0
                            || next.flags != cell.flags
                            || next.link_id != cell.link_id
                            || self
                                .cell_colors(
                                    next,
                                    selection.is_some_and(|s| s.contains(col + cells, row, cols))
                                        || self.search_match(col + cells, row),
                                )
                                .0
                                != fg
                        {
                            break;
                        }
                        text[len] = if next.ch == '\0' { b' ' } else { next.ch as u8 };
                        len += 1;
                        cells += 1;
                    }
                }
                unsafe {
                    XftDrawStringUtf8(
                        self.draw,
                        &color,
                        font,
                        x,
                        baseline,
                        text.as_ptr(),
                        len as i32,
                    );
                    if cell.flags & BOLD != 0 {
                        XftDrawStringUtf8(
                            self.draw,
                            &color,
                            font,
                            x + 1,
                            baseline,
                            text.as_ptr(),
                            len as i32,
                        );
                    }
                    let width = (cells.max(cell.width as usize) as i32 * self.state.cw) as u32;
                    if cell.flags & UNDERLINE != 0 || cell.link_id != 0 {
                        XftDrawRect(self.draw, &color, x, baseline + 2, width, 1);
                    }
                    if cell.flags & STRIKE != 0 {
                        XftDrawRect(self.draw, &color, x, y + self.state.ch / 2, width, 1);
                    }
                }
                col += cells;
            }
            self.draw_images(terminal, offset, row, 2);
            if let Some(((column, _), shape)) = cursor.filter(|((_, y), _)| *y == row) {
                self.draw_cursor(terminal, column.min(cols - 1), row, y, shape);
            }
        }
        self.last_cursor = cursor;
        self.graphics_revision = terminal.graphics().revision();
        self.graphics_alternate = terminal.modes().alternate_screen;
        terminal.clear_dirty();
    }
    fn draw_cursor(
        &mut self,
        terminal: &Terminal,
        column: usize,
        row: usize,
        y: i32,
        shape: CursorShape,
    ) {
        let cell = terminal.cell(column, row);
        let rgb = if self.state.focused {
            self.cursor_color
        } else {
            ((self.cursor_color & 0xfefefe) >> 1) + ((self.background & 0xfefefe) >> 1)
        };
        let color = self.color(rgb);
        let x = self.padding + column as i32 * self.state.cw;
        let (top, width, height) = match shape {
            CursorShape::Block => (
                y,
                self.state.cw * i32::from(cell.width.max(1)),
                self.state.ch,
            ),
            CursorShape::Underline => (y + self.state.ch - 2, self.state.cw, 2),
            CursorShape::Bar => (y, self.state.cw.min(2), self.state.ch),
        };
        unsafe { XftDrawRect(self.draw, &color, x, top, width as u32, height as u32) };
        if shape == CursorShape::Block && cell.width > 0 && !self.text_hidden(cell) {
            let contrast =
                if ((rgb >> 16) * 299 + ((rgb >> 8) & 255) * 587 + (rgb & 255) * 114) >= 128000 {
                    0x000000
                } else {
                    0xffffff
                };
            let fg = self.color(contrast);
            let font = if cell.flags & ITALIC != 0 {
                self.fonts.italic_glyph(cell.ch)
            } else {
                self.glyph_font(cell.ch)
            };
            let mut bytes = [0; zterm::MAX_GRAPHEME_BYTES];
            let mut len = 0;
            for ch in terminal.cell_chars(cell) {
                if ch != '\0' {
                    len += ch.encode_utf8(&mut bytes[len..]).len();
                }
            }
            let baseline = y + unsafe { (*self.font).ascent } + 1;
            unsafe {
                XftDrawStringUtf8(
                    self.draw,
                    &fg,
                    font,
                    x,
                    baseline,
                    bytes.as_ptr(),
                    len as i32,
                )
            };
        }
    }
}
