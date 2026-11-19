use super::*;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

impl State {
    pub(super) fn previous_cell(&self) -> Option<(usize, usize)> {
        if self.screen.x == 0 && !self.screen.wrap {
            return None;
        }
        let y = self.screen.y;
        let mut x = if self.screen.wrap {
            self.screen.x
        } else {
            self.screen.x - 1
        };
        if self.screen.row(y)[x].width == 0 && x != 0 {
            x -= 1;
        }
        Some((x, y))
    }

    pub(super) fn remember_grapheme(&mut self, x: usize, y: usize, writes: usize) {
        self.last_grapheme = Some((x, y));
        self.grapheme_overflow = false;
        self.grapheme_writes = self.grapheme_writes.saturating_add(writes);
    }

    fn intern_grapheme(&mut self, text: &str) -> Option<u32> {
        if let Some(id) = self.graphemes.intern(text) {
            return Some(id);
        }
        // A full live table cannot cause a history scan for every input scalar.
        // Retry collection only after a bounded amount of new cell output.
        if self.grapheme_writes >= self.grapheme_collect_after {
            self.graphemes.collect(
                self.screen
                    .cells
                    .iter()
                    .chain(self.primary.iter().flat_map(|s| &s.cells))
                    .chain(self.inactive_alternate.iter().flat_map(|s| &s.cells))
                    .chain(self.history.iter().flat_map(|row| &row.cells))
                    .map(|cell| cell.grapheme_id),
            );
            self.grapheme_collect_after = self.grapheme_writes.saturating_add(4096);
            return self.graphemes.intern(text);
        }
        None
    }

    fn overflow_grapheme(&mut self, x: usize, y: usize) {
        let index = self.screen.index(x, y);
        let cell = &mut self.screen.cells[index];
        cell.ch = '\u{fffd}';
        cell.combining = ['\0'; 2];
        cell.grapheme_id = 0;
        let width = usize::from(cell.width.max(1));
        self.screen.dirty[y] = true;
        self.grapheme_overflow = true;
        self.erase_sixel_span(y, x, x + width);
    }

    /// Positive-width continuation belongs to the last emitted cluster.
    /// Ordinary combining input also retains the traditional previous-cell
    /// behavior after cursor addressing. Chunk boundaries and SGR do not split.
    pub(super) fn extend_grapheme(&mut self, ch: char) -> bool {
        let addressed = self.last_grapheme.is_none() && UnicodeWidthChar::width(ch) == Some(0);
        if addressed {
            self.grapheme_overflow = false;
            self.last_grapheme = self.previous_cell();
        }
        let original_cursor = (self.screen.x, self.screen.y, self.screen.wrap);
        let Some((mut x, mut y)) = self.last_grapheme else {
            return false;
        };
        if x >= self.screen.cols || y >= self.screen.rows {
            self.last_grapheme = None;
            return false;
        }
        let old = self.screen.row(y)[x];
        let old_width = usize::from(old.width.max(1));
        let expected_x = (x + old_width).min(self.screen.cols - 1);
        let expected_wrap = self.autowrap && x + old_width >= self.screen.cols;
        if !addressed
            && (self.screen.y != y
                || self.screen.x != expected_x
                || self.screen.wrap != expected_wrap)
        {
            self.last_grapheme = None;
            return false;
        }
        if self.grapheme_overflow {
            return UnicodeWidthChar::width(ch) == Some(0);
        }
        if old.ch.is_ascii() && ch.is_ascii() {
            return false;
        }
        let mut buffer = [0u8; crate::MAX_GRAPHEME_BYTES + 4];
        let mut length = 0;
        for scalar in self.graphemes.chars(&old).chain(core::iter::once(ch)) {
            let bytes = scalar.len_utf8();
            if length + bytes > buffer.len() {
                self.overflow_grapheme(x, y);
                return true;
            }
            scalar.encode_utf8(&mut buffer[length..length + bytes]);
            length += bytes;
        }
        let text = core::str::from_utf8(&buffer[..length]).expect("encoded scalar sequence");
        if text.graphemes(true).count() != 1 {
            return false;
        }
        if text.len() > crate::MAX_GRAPHEME_BYTES {
            self.overflow_grapheme(x, y);
            return true;
        }
        let Some(id) = self.intern_grapheme(text) else {
            self.overflow_grapheme(x, y);
            return true;
        };
        let mut width = text.width().clamp(1, 2).min(self.screen.cols);
        let style = Style {
            fg: old.fg,
            bg: old.bg,
            flags: old.flags,
            palette_indices: old.palette_indices,
            link_id: old.link_id,
        };
        self.erase_sixel_span(y, x, x + old_width);
        self.screen.clear_range(y, x, x + old_width, style.blank());
        if x + width > self.screen.cols {
            if self.autowrap && self.screen.cols > 1 {
                // A late VS16 or second regional indicator may make a narrow
                // margin cell wide. Move the entire original cluster.
                self.screen.x = x;
                self.screen.y = y;
                self.screen.wrap = false;
                self.soft_wrap(x);
                x = self.screen.x;
                y = self.screen.y;
            } else {
                width = self.screen.cols - x;
            }
        }
        self.erase_sixel_span(y, x, x + width);
        let mut cell = old;
        cell.width = width as u8;
        cell.grapheme_id = id;
        cell.combining = ['\0'; 2];
        for (slot, scalar) in cell.combining.iter_mut().zip(text.chars().skip(1)) {
            *slot = scalar;
        }
        let cells = self.screen.overwrite_span(y, x, x + width, style.blank());
        cells[0] = cell;
        if width == 2 {
            cells[1] = style.cell('\0', 0);
        }
        self.screen.dirty[y] = true;
        self.screen.x = (x + width).min(self.screen.cols - 1);
        self.screen.wrap = self.autowrap && x + width >= self.screen.cols;
        if addressed && width == old_width && y == original_cursor.1 {
            self.screen.x = original_cursor.0;
            self.screen.wrap = original_cursor.2;
        }
        self.last_grapheme = Some((x, y));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_keeps_primary_cached_alternate_and_history_clusters() {
        let mut terminal = Terminal::new(8, 2, 8);
        terminal.feed("👩🏽‍💻".as_bytes());
        let primary = terminal.cell(0, 0).grapheme_id;
        terminal.feed("\x1b[?1049h🇺🇳\x1b[?1049l".as_bytes());
        let alternate = terminal.state.inactive_alternate.as_ref().unwrap().row(0)[0].grapheme_id;
        for index in 0..20_000 {
            let text = alloc::format!("{index:05}{}", "x".repeat(220));
            if terminal.state.graphemes.intern(&text).is_none() {
                break;
            }
        }
        terminal.state.grapheme_collect_after = 0;
        terminal.feed("X\u{308}".as_bytes());
        assert_eq!(terminal.state.graphemes.get(primary), Some("👩🏽‍💻"));
        assert_eq!(terminal.state.graphemes.get(alternate), Some("🇺🇳"));
        terminal.feed(b"\r\nline\r\nnext");
        assert_eq!(terminal.history_len(), 1);
        let history_id = terminal.view_cell(0, 0, 1).grapheme_id;
        terminal.state.graphemes.collect(
            terminal
                .state
                .screen
                .cells
                .iter()
                .chain(terminal.state.history.iter().flat_map(|row| &row.cells))
                .chain(
                    terminal
                        .state
                        .inactive_alternate
                        .iter()
                        .flat_map(|s| &s.cells),
                )
                .map(|cell| cell.grapheme_id),
        );
        assert_eq!(terminal.state.graphemes.get(history_id), Some("👩🏽‍💻"));
        assert_eq!(terminal.state.graphemes.get(alternate), Some("🇺🇳"));
    }
}
