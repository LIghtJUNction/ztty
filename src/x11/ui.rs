//! Drawing for transient overlays and native UI scenes.
use super::*;

impl Window {
    /// Temporary search/command UI belongs to the view, never to PTY output.
    pub fn clipboard_menu_bounds(&self, x: i32, y: i32) -> crate::layout::Rect {
        let width = (self.cw * 28 + 18).min(self.width).max(1);
        let height = (2 * (self.ch + 8) + 2).min(self.height).max(1);
        crate::layout::Rect {
            x: x.clamp(0, (self.width - width).max(0)),
            y: y.clamp(0, (self.height - height).max(0)),
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
            self.background,
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
            let y = bounds.y + 1 + row as i32 * (self.ch + 8);
            if y + self.ch + 8 > bounds.y + bounds.height {
                break;
            }
            if row == selected && enabled[row] {
                self.fill_background(
                    self.selection_colors.1,
                    false,
                    bounds.x + 1,
                    y,
                    (bounds.width - 2).max(0) as u32,
                    (self.ch + 8) as u32,
                );
            }
            let color = if !enabled[row] {
                0x657080
            } else if row == selected {
                self.selection_colors.0
            } else {
                0xcdd6e4
            };
            let right = bounds.x + bounds.width - 9;
            let x = (right - self.overlay_width(shortcut)).max(bounds.x + 9);
            self.overlay_text_color(
                label,
                bounds.x + 9,
                y + 4,
                (x - self.cw).max(bounds.x + 9),
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
        let y = (self.height - self.padding - self.ch).max(0);
        self.fill_background(
            self.background,
            false,
            0,
            y,
            self.width as u32,
            (self.height - y) as u32,
        );
        self.overlay_line(text, y);
        self.damage(y, self.height);
    }

    pub fn overlay_lines(&mut self, lines: &[String]) {
        if self.suspended {
            return;
        }
        let count = lines
            .len()
            .min(((self.height - self.padding).max(self.ch) / self.ch) as usize);
        let y = (self.height - self.padding - self.ch * count as i32).max(0);
        self.fill_background(
            self.background,
            false,
            0,
            y,
            self.width as u32,
            (self.height - y) as u32,
        );
        for (row, line) in lines.iter().take(count).enumerate() {
            self.overlay_line(line, y + row as i32 * self.ch);
        }
        self.damage(y, self.height);
    }

    fn overlay_line(&mut self, text: &str, y: i32) {
        self.overlay_text(text, self.padding, y, self.width - self.padding);
    }

    fn overlay_text(&mut self, text: &str, x: i32, y: i32, right: i32) {
        self.overlay_text_color(text, x, y, right, 0xcdd6e4);
    }

    fn overlay_text_color(&mut self, text: &str, mut x: i32, y: i32, right: i32, rgb: u32) {
        if self.gpu_text(
            text,
            crate::layout::Rect {
                x,
                y,
                width: (right - x).max(0),
                height: self.ch,
            },
            rgb,
        ) {
            return;
        }
        let color = self.color(rgb);
        let baseline = y + unsafe { (*self.font).ascent } + 1;
        for ch in text.chars().take(1024) {
            let font = self.glyph_font(ch);
            let mut bytes = [0; 4];
            let text = ch.encode_utf8(&mut bytes);
            let mut metrics: GlyphInfo = unsafe { core::mem::zeroed() };
            unsafe {
                XftTextExtentsUtf8(
                    self.display,
                    font,
                    text.as_ptr(),
                    text.len() as i32,
                    &mut metrics,
                );
            }
            if x + i32::from(metrics.x_off).max(1) > right {
                break;
            }
            unsafe {
                XftDrawStringUtf8(
                    self.draw,
                    &color,
                    font,
                    x,
                    baseline,
                    text.as_ptr(),
                    text.len() as i32,
                );
            }
            x += i32::from(metrics.x_off);
        }
    }

    fn overlay_width(&mut self, text: &str) -> i32 {
        if let Some(width) = self.gpu_overlay_width(text) {
            return width;
        }
        let mut width: i32 = 0;
        for ch in text.chars() {
            let font = self.glyph_font(ch);
            let mut bytes = [0; 4];
            let text = ch.encode_utf8(&mut bytes);
            let mut metrics: GlyphInfo = unsafe { core::mem::zeroed() };
            unsafe {
                XftTextExtentsUtf8(
                    self.display,
                    font,
                    text.as_ptr(),
                    text.len() as i32,
                    &mut metrics,
                );
            }
            width = width.saturating_add(i32::from(metrics.x_off).max(1));
        }
        width
    }

    /// A small view-local affordance; no terminal rows are reserved for it.
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
                self.width,
                self.height,
                self.padding,
                self.ch,
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
                self.background,
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

    pub fn native_ui_geometry(&self) -> ztty::native_ui::Geometry {
        ztty::native_ui::Geometry {
            width: self.width,
            height: self.height,
            cell_width: self.cw,
            cell_height: self.ch,
            padding: self.padding,
        }
    }

    fn native_ui_fill(&mut self, rect: crate::layout::Rect, rgb: u32) {
        if rect.width <= 0 || rect.height <= 0 {
            return;
        }
        self.fill_background(
            rgb,
            false,
            rect.x,
            rect.y,
            rect.width as u32,
            rect.height as u32,
        );
    }

    fn native_ui_text(&mut self, text: &str, rect: crate::layout::Rect, rgb: u32) {
        if rect.width <= 0 || rect.height <= 0 {
            return;
        }
        if self.gpu_text(text, rect, rgb) {
            return;
        }
        let clip = ClipRectangle {
            x: 0,
            y: 0,
            width: rect.width.min(i32::from(u16::MAX)) as u16,
            height: rect.height.min(i32::from(u16::MAX)) as u16,
        };
        // Glyph ink, including fallback/combining marks, may exceed its advance.
        // Restrict these new native UI draws without clipping terminal/palette draws.
        if unsafe { XftDrawSetClipRectangles(self.draw, rect.x, rect.y, &clip, 1) } == 0 {
            return;
        }

        let color = self.color(rgb);
        let right = rect.x + rect.width;
        let rows = (rect.height / self.ch).max(0) as usize;
        let draw = self.draw;
        for (row, line) in text.split('\n').take(rows.min(32)).enumerate() {
            let mut x = rect.x;
            let baseline = rect.y + row as i32 * self.ch + unsafe { (*self.font).ascent } + 1;
            // One Xft draw per font run, with bounded stack storage. Fallback
            // fonts share the terminal's existing glyph/font cache.
            let mut run = [0u8; 4096];
            let mut used = 0usize;
            let mut run_font = core::ptr::null_mut();
            let mut run_x = x;
            let flush = |font, x, bytes: &[u8]| {
                if !bytes.is_empty() {
                    unsafe {
                        XftDrawStringUtf8(
                            draw,
                            &color,
                            font,
                            x,
                            baseline,
                            bytes.as_ptr(),
                            bytes.len() as i32,
                        );
                    }
                }
            };
            for ch in line.chars().filter(|ch| !ch.is_control()).take(4096) {
                let font = self.glyph_font(ch);
                let mut bytes = [0; 4];
                let text = ch.encode_utf8(&mut bytes);
                let mut metrics: GlyphInfo = unsafe { core::mem::zeroed() };
                unsafe {
                    XftTextExtentsUtf8(
                        self.display,
                        font,
                        text.as_ptr(),
                        text.len() as i32,
                        &mut metrics,
                    );
                }
                if x + i32::from(metrics.x_off).max(1) > right {
                    break;
                }
                if run_font != font || used + text.len() > run.len() {
                    flush(run_font, run_x, &run[..used]);
                    used = 0;
                    run_font = font;
                    run_x = x;
                }
                run[used..used + text.len()].copy_from_slice(text.as_bytes());
                used += text.len();
                x += i32::from(metrics.x_off);
            }
            flush(run_font, run_x, &run[..used]);
        }
        unsafe {
            XftDrawSetClip(self.draw, core::ptr::null_mut());
        }
    }

    /// Use the same generation-keyed XRender upload as Kitty/Sixel placements.
    /// Scene references reuse that upload; scaling allocates no second bitmap.
    fn native_ui_image(
        &mut self,
        terminal: &Terminal,
        image_id: u32,
        generation: Option<u64>,
        rect: crate::layout::Rect,
    ) -> bool {
        let Some(image) = terminal.graphics().image(image_id) else {
            return false;
        };
        if generation.is_some_and(|generation| generation != image.generation)
            || image.width == 0
            || image.height == 0
            || rect.width <= 0
            || rect.height <= 0
        {
            return false;
        }
        let (width, height) = if i64::from(rect.width) * i64::from(image.height)
            <= i64::from(rect.height) * i64::from(image.width)
        {
            (
                rect.width,
                (i64::from(rect.width) * i64::from(image.height) / i64::from(image.width)).max(1)
                    as i32,
            )
        } else {
            (
                (i64::from(rect.height) * i64::from(image.width) / i64::from(image.height)).max(1)
                    as i32,
                rect.height,
            )
        };
        if self.gpu_native_image(
            image_id,
            image,
            crate::layout::Rect {
                width,
                height,
                ..rect
            },
        ) {
            return true;
        }
        let Some(picture) = self.image_picture(image_id, image) else {
            return false;
        };
        let fixed = |source: u32, destination: i32| {
            ((u64::from(source) << 16) / destination as u64).min(i32::MAX as u64) as i32
        };
        let transform = Transform {
            matrix: [
                [fixed(image.width, width), 0, 0],
                [0, fixed(image.height, height), 0],
                [0, 0, 1 << 16],
            ],
        };
        unsafe {
            XRenderSetPictureTransform(self.display, picture, &transform);
            XRenderComposite(
                self.display,
                3,
                picture,
                0,
                XftDrawPicture(self.draw),
                0,
                0,
                0,
                0,
                rect.x,
                rect.y,
                width as u32,
                height as u32,
            );
        }
        true
    }

    pub fn native_ui(
        &mut self,
        terminal: &Terminal,
        view: &ztty::native_ui::View,
        interactive: bool,
    ) {
        use crate::layout::Rect;
        use zterm::ui::{Kind, TextStyle};
        if self.suspended {
            return;
        }
        let Some(layout) = &view.layout else {
            return;
        };
        let Some(scene) = terminal.ui_scene() else {
            return;
        };
        if layout.stamp.change != terminal.ui_revision() {
            return;
        }
        let panel = layout.panel;
        self.native_ui_fill(panel, 0x354457);
        self.native_ui_fill(
            Rect {
                x: panel.x + 1,
                y: panel.y + 1,
                width: panel.width - 2,
                height: panel.height - 2,
            },
            0x121a24,
        );
        let header = if interactive {
            if view.focused_node().is_some() {
                "ztty UI  ·  Tab selects  ·  Enter activates  ·  Esc returns"
            } else {
                "ztty UI  ·  Click a button to focus"
            }
        } else {
            "ztty UI  ·  Read-only preview"
        };
        self.native_ui_text(
            header,
            Rect {
                x: panel.x + 16,
                y: panel.y + 10,
                width: panel.width - 32,
                height: self.ch,
            },
            0x94a3b8,
        );
        for item in &layout.items {
            let Some(node) = scene.nodes.get(item.index) else {
                continue;
            };
            let rect = item.rect;
            match &node.kind {
                Kind::Text { text, style } => {
                    let color = match style {
                        TextStyle::Heading => 0xe8eef8,
                        TextStyle::Muted => 0x94a3b8,
                        TextStyle::Code => 0x8ab4f8,
                        TextStyle::Body => 0xcdd6e4,
                    };
                    self.native_ui_text(text, rect, color);
                }
                Kind::Progress { value, label } => {
                    self.native_ui_text(
                        label,
                        Rect {
                            height: self.ch.min(rect.height),
                            ..rect
                        },
                        0xcdd6e4,
                    );
                    if rect.height >= self.ch + 10 {
                        let bar = Rect {
                            y: rect.y + self.ch + 6,
                            height: 4,
                            ..rect
                        };
                        self.native_ui_fill(bar, 0x314155);
                        self.native_ui_fill(
                            Rect {
                                width: (i64::from(bar.width) * i64::from(*value) / 10000) as i32,
                                ..bar
                            },
                            0x8ab4f8,
                        );
                    }
                }
                Kind::Button { label, enabled, .. } => {
                    if rect.height < self.ch + 16 {
                        continue;
                    }
                    let active = interactive && *enabled;
                    self.native_ui_fill(
                        rect,
                        if view.focused_node() == Some(node.id) {
                            0x8ab4f8
                        } else {
                            0x40536b
                        },
                    );
                    self.native_ui_fill(
                        Rect {
                            x: rect.x + 1,
                            y: rect.y + 1,
                            width: rect.width - 2,
                            height: rect.height - 2,
                        },
                        if active { 0x23354a } else { 0x202731 },
                    );
                    self.native_ui_text(
                        label,
                        Rect {
                            x: rect.x + 12,
                            y: rect.y + 8,
                            width: rect.width - 24,
                            height: self.ch,
                        },
                        if active { 0xdce9fb } else { 0x7c8999 },
                    );
                }
                Kind::Image {
                    image_id,
                    generation,
                    alt,
                    cols,
                    rows,
                } => {
                    let image_rect = Rect {
                        width: rect.width.min(i32::from(*cols).saturating_mul(self.cw)),
                        height: rect.height.min(i32::from(*rows).saturating_mul(self.ch)),
                        ..rect
                    };
                    if !self.native_ui_image(terminal, *image_id, *generation, image_rect) {
                        self.native_ui_fill(image_rect, 0x202b3a);
                        self.native_ui_text(
                            alt,
                            Rect {
                                x: image_rect.x + 8,
                                y: image_rect.y + 8,
                                width: image_rect.width - 16,
                                height: image_rect.height - 16,
                            },
                            0x94a3b8,
                        );
                    }
                }
                Kind::Column { .. } | Kind::Row { .. } => {}
            }
        }
        self.damage(panel.y, panel.y + panel.height);
    }
}
