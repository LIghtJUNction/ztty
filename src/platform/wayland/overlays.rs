use super::*;

impl Window {
    pub fn clipboard_menu_bounds(&self, x: i32, y: i32) -> crate::layout::Rect {
        let width = (self.state.cw * 28 + 18).min(self.state.width).max(1);
        let height = (2 * (self.state.ch + 8) + 2).min(self.state.height).max(1);
        crate::layout::Rect {
            x: x.clamp(0, (self.state.width - width).max(0)),
            y: y.clamp(0, (self.state.height - height).max(0)),
            width,
            height,
        }
    }
    pub fn clipboard_menu(
        &mut self,
        bounds: crate::layout::Rect,
        selected: usize,
        enabled: [bool; 2],
        shortcuts: [&str; 2],
    ) {
        if self.suspended {
            return;
        }
        self.fill_background(
            0x5f7287,
            false,
            bounds.x,
            bounds.y,
            bounds.width as u32,
            bounds.height as u32,
        );
        self.fill_background(
            self.appearance.theme.background,
            false,
            bounds.x + 1,
            bounds.y + 1,
            (bounds.width - 2).max(0) as u32,
            (bounds.height - 2).max(0) as u32,
        );
        for (row, (label, shortcut)) in [("Copy", shortcuts[0]), ("Paste", shortcuts[1])]
            .into_iter()
            .enumerate()
        {
            let y = bounds.y + 1 + row as i32 * (self.state.ch + 8);
            if y + self.state.ch + 8 > bounds.y + bounds.height {
                break;
            }
            if row == selected && enabled[row] {
                self.fill_background(
                    self.appearance.selection_background,
                    false,
                    bounds.x + 1,
                    y,
                    (bounds.width - 2).max(0) as u32,
                    (self.state.ch + 8) as u32,
                );
            }
            let color = if !enabled[row] {
                0x657080
            } else if row == selected {
                self.appearance.selection_foreground
            } else {
                0xcdd6e4
            };
            let right = bounds.x + bounds.width - 9;
            let x = (right - self.overlay_width(shortcut)).max(bounds.x + 9);
            self.overlay_text_color(
                label,
                bounds.x + 9,
                y + 4,
                (x - self.state.cw).max(bounds.x + 9),
                color,
            );
            self.overlay_text_color(shortcut, x, y + 4, right, color);
        }
        self.damage(bounds.y, bounds.y + bounds.height);
    }
    pub fn overlay(&mut self, text: &str) {
        if self.suspended {
            return;
        }
        let y = (self.state.height - self.appearance.padding - self.state.ch).max(0);
        self.fill_background(
            self.appearance.theme.background,
            false,
            0,
            y,
            self.state.width as u32,
            (self.state.height - y) as u32,
        );
        self.overlay_line(text, y);
        self.damage(y, self.state.height);
    }
    pub fn overlay_lines(&mut self, lines: &[String]) {
        if self.suspended {
            return;
        }
        let count = lines.len().min(
            ((self.state.height - self.appearance.padding).max(self.state.ch) / self.state.ch)
                as usize,
        );
        let y = (self.state.height - self.appearance.padding - self.state.ch * count as i32).max(0);
        self.fill_background(
            self.appearance.theme.background,
            false,
            0,
            y,
            self.state.width as u32,
            (self.state.height - y) as u32,
        );
        for (row, line) in lines.iter().take(count).enumerate() {
            self.overlay_line(line, y + row as i32 * self.state.ch);
        }
        self.damage(y, self.state.height);
    }
    pub(super) fn overlay_line(&mut self, text: &str, y: i32) {
        self.overlay_text(
            text,
            self.appearance.padding,
            y,
            self.state.width - self.appearance.padding,
        );
    }
    pub(super) fn overlay_text(&mut self, text: &str, x: i32, y: i32, right: i32) {
        self.overlay_text_color(text, x, y, right, 0xcdd6e4);
    }
    pub fn scroll_prompt(
        &mut self,
        new_output: bool,
        overlay_rows: usize,
    ) -> Option<crate::layout::Rect> {
        use ztty::scroll_prompt::{INSET_X, INSET_Y, place};
        if self.suspended {
            return None;
        }
        let labels = if new_output {
            ["New output · Back to live ↓", "New · Live ↓", "↓"]
        } else {
            ["Back to live ↓", "Live ↓", "↓"]
        };
        for text in labels {
            let width = self.overlay_width(text);
            let Some(rect) = place(
                self.state.width,
                self.state.height,
                self.appearance.padding,
                self.state.ch,
                width,
                overlay_rows,
            ) else {
                continue;
            };
            self.fill_background(
                if new_output { 0x89b4fa } else { 0x64748b },
                false,
                rect.x,
                rect.y,
                rect.width as u32,
                rect.height as u32,
            );
            self.fill_background(
                self.appearance.theme.background,
                false,
                rect.x + 1,
                rect.y + 1,
                (rect.width - 2) as u32,
                (rect.height - 2) as u32,
            );
            self.overlay_text(
                text,
                rect.x + INSET_X,
                rect.y + INSET_Y,
                rect.x + rect.width - INSET_X,
            );
            self.damage(rect.y, rect.y + rect.height);
            return Some(rect);
        }
        None
    }
}
