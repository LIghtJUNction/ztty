use super::{MAX_TEXT_BYTES, Rect, Renderer, Style, TextCell};
use alloc::{string::String, vec::Vec};
use zterm::{
    BLINK, BOLD, Cell, CursorShape, DIM, EXPLICIT_BG, HIDDEN, INVERSE, ITALIC, RAPID_BLINK, STRIKE,
    Terminal, UNDERLINE,
};

/// Geometry and colors belong to the view. The terminal continues to own text,
/// selection coordinates, image placements and output history.
#[derive(Clone, Copy)]
pub struct GridStyle {
    pub padding: f32,
    pub cell_width: f32,
    pub cell_height: f32,
    pub ascent: f32,
    pub background: u32,
    pub background_alpha: u8,
    pub selection_foreground: u32,
    pub selection_background: u32,
    pub cursor: Option<((usize, usize), CursorShape)>,
    pub cursor_color: u32,
    pub slow_blink_visible: bool,
    pub rapid_blink_visible: bool,
}

fn colors(cell: &Cell, selected: bool, style: &GridStyle) -> (u32, u32, bool) {
    let (mut foreground, mut background) = if cell.flags & INVERSE != 0 {
        (cell.bg, cell.fg)
    } else {
        (cell.fg, cell.bg)
    };
    if cell.flags & DIM != 0 {
        foreground = ((foreground & 0xfefefe) >> 1) + ((background & 0xfefefe) >> 1);
    }
    if selected {
        foreground = style.selection_foreground;
        background = style.selection_background;
    }
    if cell.flags & HIDDEN != 0 {
        foreground = background;
    }
    (
        foreground,
        background,
        !selected && cell.flags & (INVERSE | EXPLICIT_BG) == 0,
    )
}

fn hidden(cell: &Cell, style: &GridStyle) -> bool {
    cell.ch == '\u{10eeee}'
        || cell.flags & HIDDEN != 0
        || (cell.flags & BLINK != 0 && !style.slow_blink_visible)
        || (cell.flags & RAPID_BLINK != 0 && !style.rapid_blink_visible)
}

impl Renderer {
    /// Render one complete terminal viewport. Add overlays after this call and
    /// present once after the complete frame is ready.
    pub fn draw_terminal(
        &mut self,
        terminal: &Terminal,
        history_offset: usize,
        style: GridStyle,
        selected: impl Fn(usize, usize) -> bool,
    ) -> Result<(), &'static str> {
        if !style.cell_width.is_finite()
            || !style.cell_height.is_finite()
            || style.cell_width <= 0.0
            || style.cell_height <= 0.0
        {
            return Err("invalid terminal rendering geometry");
        }
        self.begin_frame(style.background, style.background_alpha)?;
        let viewport = Rect {
            x: style.padding,
            y: style.padding,
            width: terminal.cols() as f32 * style.cell_width,
            height: terminal.rows() as f32 * style.cell_height,
        };
        self.clip(Some(viewport))?;
        self.retain_terminal_images(terminal, history_offset)?;
        for row in 0..terminal.rows() {
            self.background_row(terminal, history_offset, row, &style, &selected, false)?;
        }
        self.image_layer(terminal, history_offset, &style, 0)?;
        if terminal
            .graphics_draws(history_offset)
            .any(|image| image.z < -1_073_741_824)
        {
            for row in 0..terminal.rows() {
                self.background_row(terminal, history_offset, row, &style, &selected, true)?;
            }
        }
        self.image_layer(terminal, history_offset, &style, 1)?;
        for row in 0..terminal.rows() {
            self.text_row(terminal, history_offset, row, &style, &selected, None)?;
        }
        self.image_layer(terminal, history_offset, &style, 2)?;
        if let Some(((column, row), shape)) = style.cursor
            && column < terminal.cols()
            && row < terminal.rows()
        {
            let x = style.padding + column as f32 * style.cell_width;
            let y = style.padding + row as f32 * style.cell_height;
            let rectangle = match shape {
                CursorShape::Underline => Rect {
                    x,
                    y: y + style.cell_height - 2.0,
                    width: style.cell_width,
                    height: 2.0,
                },
                CursorShape::Bar => Rect {
                    x,
                    y,
                    width: 2.0,
                    height: style.cell_height,
                },
                CursorShape::Block => Rect {
                    x,
                    y,
                    width: style.cell_width
                        * terminal.view_cell(column, row, history_offset).width.max(1) as f32,
                    height: style.cell_height,
                },
            };
            self.draw_rect(rectangle, style.cursor_color, 255)?;
            if shape == CursorShape::Block {
                self.clip(Some(rectangle))?;
                let rgb = style.cursor_color;
                let foreground = if ((rgb >> 16) * 299
                    + ((rgb >> 8) & 255) * 587
                    + (rgb & 255) * 114)
                    >= 128_000
                {
                    0x000000
                } else {
                    0xffffff
                };
                self.text_row(
                    terminal,
                    history_offset,
                    row,
                    &style,
                    &selected,
                    Some(foreground),
                )?;
                self.clip(Some(viewport))?;
            }
        }
        self.clip(None)
    }

    fn retain_terminal_images(
        &mut self,
        terminal: &Terminal,
        offset: usize,
    ) -> Result<(), &'static str> {
        let retired: Vec<u64> = self.images.iter().filter_map(|(key, cached)| {
            let placed = terminal.graphics_draws(offset).any(|draw|
                u64::from(draw.image_id) == *key && draw.image.generation == cached.revision);
            let in_ui = terminal.ui_scene().is_some_and(|scene| scene.nodes.iter().any(|node| {
                matches!(&node.kind, zterm::ui::Kind::Image { image_id, generation, .. }
                    if u64::from(*image_id) == *key
                        && generation.is_none_or(|g| g == cached.revision)
                        && terminal.graphics().image(*image_id).is_some_and(|image| image.generation == cached.revision))
            }));
            (!placed && !in_ui).then_some(*key)
        }).collect();
        for key in retired {
            self.remove_image(key)?;
        }
        Ok(())
    }

    fn image_layer(
        &mut self,
        terminal: &Terminal,
        offset: usize,
        style: &GridStyle,
        layer: u8,
    ) -> Result<(), &'static str> {
        let viewport = Rect {
            x: style.padding,
            y: style.padding,
            width: terminal.cols() as f32 * style.cell_width,
            height: terminal.rows() as f32 * style.cell_height,
        };
        for draw in terminal.graphics_draws(offset) {
            let current = if draw.z < -1_073_741_824 {
                0
            } else if draw.z < 0 {
                1
            } else {
                2
            };
            if current != layer {
                continue;
            }
            if let Some(clip) = draw.clip {
                // Unicode placeholders reveal only their occupied text cells.
                // Keep the full source transform, but intersect the tile with
                // the viewport before submitting any image vertices.
                let left = (style.padding + clip.x as f32).max(viewport.x);
                let top = (style.padding + clip.y as f32).max(viewport.y);
                let right = (style.padding + clip.x as f32 + clip.width as f32)
                    .min(viewport.x + viewport.width);
                let bottom = (style.padding + clip.y as f32 + clip.height as f32)
                    .min(viewport.y + viewport.height);
                if right <= left || bottom <= top {
                    continue;
                }
                self.clip(Some(Rect {
                    x: left,
                    y: top,
                    width: right - left,
                    height: bottom - top,
                }))?;
            }
            self.draw_image(
                u64::from(draw.image_id),
                draw.image.generation,
                (draw.image.width, draw.image.height),
                &draw.image.pixels,
                Rect {
                    x: draw.source.x as f32,
                    y: draw.source.y as f32,
                    width: draw.source.width as f32,
                    height: draw.source.height as f32,
                },
                Rect {
                    x: style.padding + draw.dest.x as f32,
                    y: style.padding + draw.dest.y as f32,
                    width: draw.dest.width as f32,
                    height: draw.dest.height as f32,
                },
            )?;
            if draw.clip.is_some() {
                self.clip(Some(viewport))?;
            }
        }
        Ok(())
    }

    fn background_row(
        &mut self,
        terminal: &Terminal,
        offset: usize,
        row: usize,
        style: &GridStyle,
        selected: &impl Fn(usize, usize) -> bool,
        explicit_only: bool,
    ) -> Result<(), &'static str> {
        let mut column = 0;
        while column < terminal.cols() {
            let (_, rgb, translucent) = colors(
                terminal.view_cell(column, row, offset),
                selected(column, row),
                style,
            );
            let mut end = column + 1;
            while end < terminal.cols() {
                let (_, next_rgb, next_translucent) = colors(
                    terminal.view_cell(end, row, offset),
                    selected(end, row),
                    style,
                );
                if (rgb, translucent) != (next_rgb, next_translucent) {
                    break;
                }
                end += 1;
            }
            if !translucent || (!explicit_only && rgb != style.background) {
                self.draw_rect(
                    Rect {
                        x: style.padding + column as f32 * style.cell_width,
                        y: style.padding + row as f32 * style.cell_height,
                        width: (end - column) as f32 * style.cell_width,
                        height: style.cell_height,
                    },
                    rgb,
                    if translucent {
                        style.background_alpha
                    } else {
                        255
                    },
                )?;
            }
            column = end;
        }
        Ok(())
    }

    fn text_row(
        &mut self,
        terminal: &Terminal,
        offset: usize,
        row: usize,
        style: &GridStyle,
        selected: &impl Fn(usize, usize) -> bool,
        override_color: Option<u32>,
    ) -> Result<(), &'static str> {
        // Own the buffers while drawing so draw_cells can borrow the renderer.
        // Keep them between rows and frames, including fallible shaping/GL paths.
        let mut text = core::mem::take(&mut self.row_text);
        let mut cells = core::mem::take(&mut self.row_cells);
        let result = (|| {
            let mut column = 0;
            let y = style.padding + row as f32 * style.cell_height;
            while column < terminal.cols() {
                let first = terminal.view_cell(column, row, offset);
                if first.width == 0 || hidden(first, style) {
                    column += 1;
                    continue;
                }
                let (rgb, _, _) = colors(first, selected(column, row), style);
                let start = column;
                let flags = first.flags;
                let link = first.link_id;
                text.clear();
                cells.clear();
                while column < terminal.cols() {
                    let cell = terminal.view_cell(column, row, offset);
                    if cell.width == 0 {
                        column += 1;
                        continue;
                    }
                    if cell.flags != flags
                        || cell.link_id != link
                        || colors(cell, selected(column, row), style).0 != rgb
                        || hidden(cell, style)
                    {
                        break;
                    }
                    let byte_count = terminal.grapheme(cell).map(str::len).unwrap_or(12);
                    if !text.is_empty() && text.len() + byte_count > MAX_TEXT_BYTES {
                        break;
                    }
                    cells.push(TextCell {
                        byte: text.len() as u32,
                        x: (column - start) as f32 * style.cell_width,
                        width: cell.width as f32 * style.cell_width,
                    });
                    let old_len = text.len();
                    for character in terminal.cell_chars(cell) {
                        text.push(if character == '\0' { ' ' } else { character });
                    }
                    if text.len() == old_len {
                        text.push(' ');
                    }
                    column += 1;
                }
                let foreground = override_color.unwrap_or(rgb);
                self.draw_cells(
                    &text,
                    &cells,
                    style.padding + start as f32 * style.cell_width,
                    y + style.ascent,
                    foreground,
                    Style {
                        bold: flags & BOLD != 0,
                        italic: flags & ITALIC != 0,
                    },
                )?;
                let width = (column - start) as f32 * style.cell_width;
                if flags & UNDERLINE != 0 || link != 0 {
                    self.draw_rect(
                        Rect {
                            x: style.padding + start as f32 * style.cell_width,
                            y: y + style.ascent + 2.0,
                            width,
                            height: 1.0,
                        },
                        foreground,
                        255,
                    )?;
                }
                if flags & STRIKE != 0 {
                    self.draw_rect(
                        Rect {
                            x: style.padding + start as f32 * style.cell_width,
                            y: y + style.cell_height / 2.0,
                            width,
                            height: 1.0,
                        },
                        foreground,
                        255,
                    )?;
                }
            }
            Ok(())
        })();
        text.clear();
        cells.clear();
        // Bound retained text to one shaping run and release cell capacity from
        // a larger terminal after a resize. Vec's smallest capacity is four.
        self.row_text = if text.capacity() <= MAX_TEXT_BYTES {
            text
        } else {
            String::new()
        };
        self.row_cells = if cells.capacity() <= terminal.cols().max(4).next_power_of_two() {
            cells
        } else {
            Vec::new()
        };
        result
    }
}
