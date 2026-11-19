use super::*;

impl Window {
    pub fn native_ui_geometry(&self) -> ztty::native_ui::Geometry {
        ztty::native_ui::Geometry {
            width: self.state.width,
            height: self.state.height,
            cell_width: self.state.cw,
            cell_height: self.state.ch,
            padding: self.appearance.padding,
        }
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
                height: self.state.ch,
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
                            height: self.state.ch.min(rect.height),
                            ..rect
                        },
                        0xcdd6e4,
                    );
                    if rect.height >= self.state.ch + 10 {
                        let bar = Rect {
                            y: rect.y + self.state.ch + 6,
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
                    if rect.height < self.state.ch + 16 {
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
                            height: self.state.ch,
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
                        width: rect
                            .width
                            .min(i32::from(*cols).saturating_mul(self.state.cw)),
                        height: rect
                            .height
                            .min(i32::from(*rows).saturating_mul(self.state.ch)),
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
fn gpu_rect(r: crate::layout::Rect) -> Rect {
    Rect {
        x: r.x as f32,
        y: r.y as f32,
        width: r.width.max(0) as f32,
        height: r.height.max(0) as f32,
    }
}
impl Window {
    pub(super) fn damage(&mut self, _: i32, _: i32) {}
    pub(super) fn fill_background(
        &mut self,
        rgb: u32,
        _: bool,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) {
        self.native_ui_fill(
            crate::layout::Rect {
                x,
                y,
                width: width as i32,
                height: height as i32,
            },
            rgb,
        );
    }
    pub(super) fn native_ui_fill(&mut self, r: crate::layout::Rect, rgb: u32) {
        if self.drawing
            && r.width > 0
            && r.height > 0
            && let Err(e) = self.renderer.draw_rect(gpu_rect(r), rgb, 255)
        {
            self.failure(e);
        }
    }
    pub(super) fn native_ui_text(&mut self, text: &str, r: crate::layout::Rect, rgb: u32) {
        if !self.drawing || r.width <= 0 || r.height <= 0 {
            return;
        }
        let result = (|| {
            self.renderer.clip(Some(gpu_rect(r)))?;
            for (row, line) in text
                .split('\n')
                .take((r.height / self.state.ch).clamp(0, 32) as usize)
                .enumerate()
            {
                let mut end = line.len().min(16384);
                while !line.is_char_boundary(end) {
                    end -= 1;
                }
                self.renderer.draw_text(
                    &line[..end],
                    r.x as f32,
                    r.y as f32 + self.ascent + row as f32 * self.state.ch as f32,
                    rgb,
                    Style::default(),
                )?;
            }
            self.renderer.clip(None)
        })();
        if let Err(e) = result {
            self.failure(e);
        }
    }
    pub(super) fn overlay_text_color(&mut self, text: &str, x: i32, y: i32, right: i32, rgb: u32) {
        self.native_ui_text(
            text,
            crate::layout::Rect {
                x,
                y,
                width: (right - x).max(0),
                height: self.state.ch,
            },
            rgb,
        );
    }
    pub(super) fn overlay_width(&mut self, text: &str) -> i32 {
        match self.renderer.measure_text(text, Style::default()) {
            Ok(w) => (w + 0.999) as i32,
            Err(e) => {
                self.failure(e);
                0
            }
        }
    }
    pub(super) fn native_ui_image(
        &mut self,
        t: &Terminal,
        id: u32,
        generation: Option<u64>,
        r: crate::layout::Rect,
    ) -> bool {
        if !self.drawing {
            return false;
        }
        let Some(image) = t.graphics().image(id) else {
            return false;
        };
        if generation.is_some_and(|g| g != image.generation)
            || image.width == 0
            || image.height == 0
            || r.width <= 0
            || r.height <= 0
        {
            return false;
        }
        let scale =
            (r.width as f32 / image.width as f32).min(r.height as f32 / image.height as f32);
        let target = Rect {
            width: image.width as f32 * scale,
            height: image.height as f32 * scale,
            ..gpu_rect(r)
        };
        match self.renderer.draw_image(
            u64::from(id),
            image.generation,
            (image.width, image.height),
            &image.pixels,
            Rect {
                x: 0.0,
                y: 0.0,
                width: image.width as f32,
                height: image.height as f32,
            },
            target,
        ) {
            Ok(()) => true,
            Err(e) => {
                self.failure(e);
                false
            }
        }
    }
}
