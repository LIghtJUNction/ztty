//! CoreText cell rendering and Core Graphics image composition.
use super::{Host, Id, MAX_LINES, NIL, Rect, Renderer, Sel, Size, ffi::*};
use crate::appearance::Appearance;
use alloc::{collections::BTreeMap, string::String, sync::Arc, vec::Vec};
use core::{ffi::c_void, ptr};
use zterm::{
    BOLD, Cell, CursorShape, DIM, EXPLICIT_BG, HIDDEN, INVERSE, ITALIC, STRIKE, UNDERLINE,
};

impl Renderer {
    pub(super) fn new(appearance: &Appearance) -> Result<Self, &'static str> {
        let name = appearance.font.split(':').next().unwrap_or("Menlo");
        let name = if name == "monospace" || name.is_empty() {
            "Menlo"
        } else {
            name
        };
        let points = appearance
            .font
            .split(':')
            .find_map(|s| s.strip_prefix("size="))
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|n| n.is_finite())
            .unwrap_or(11.0)
            .clamp(6.0, 72.0);
        let string = ns_string(name);
        let base = unsafe { CTFontCreateWithName(string, points, ptr::null()) };
        release(string);
        if base.is_null() {
            return Err("cannot create a CoreText font");
        }
        let mut fonts = [base; 4];
        for (i, font) in fonts.iter_mut().enumerate().skip(1) {
            let traits = ((i & 1) as u32) * 2 + (((i >> 1) & 1) as u32);
            let alternate =
                unsafe { CTFontCreateCopyWithSymbolicTraits(base, 0.0, ptr::null(), traits, 3) };
            *font = if alternate.is_null() {
                unsafe { CFRetain(base) }
            } else {
                alternate
            };
        }
        let mut glyph = 0u16;
        let mut advance = Size::default();
        unsafe {
            CTFontGetGlyphsForCharacters(base, &('M' as u16), &mut glyph, 1);
            CTFontGetAdvancesForGlyphs(base, 0, &glyph, &mut advance, 1);
        }
        let ascent = unsafe { CTFontGetAscent(base) };
        let descent = unsafe { CTFontGetDescent(base) };
        let leading = unsafe { CTFontGetLeading(base) };
        // Kitty and Sixel pixels use sRGB. Device RGB makes their displayed
        // colors depend on the monitor profile and changes captured pixels.
        let color_space = unsafe { CGColorSpaceCreateWithName(kCGColorSpaceSRGB) };
        if color_space.is_null() {
            for font in fonts {
                cf_release(font);
            }
            return Err("cannot create the CoreGraphics color space");
        }
        Ok(Self {
            fonts,
            lines: BTreeMap::new(),
            color_space,
            cell_width: (advance.width.max(points * 0.4) + 0.999) as u32 as f64,
            cell_height: (ascent + descent + leading.max(0.0) + 1.999) as u32 as f64,
            descent: (descent + 0.999) as u32 as f64,
        })
    }
    pub(super) fn make_line(&self, text: &str, style: u8) -> Id {
        let string = unsafe {
            CFStringCreateWithBytes(NIL, text.as_ptr(), text.len() as isize, 0x08000100, false)
        };
        if string.is_null() {
            return NIL;
        }
        let keys = unsafe {
            [
                kCTFontAttributeName,
                kCTForegroundColorFromContextAttributeName,
            ]
        };
        let values = unsafe { [self.fonts[style as usize], kCFBooleanTrue] };
        let dictionary = unsafe {
            CFDictionaryCreate(
                NIL,
                keys.as_ptr(),
                values.as_ptr(),
                2,
                ptr::addr_of!(kCFTypeDictionaryKeyCallBacks).cast(),
                ptr::addr_of!(kCFTypeDictionaryValueCallBacks).cast(),
            )
        };
        let attributed = unsafe { CFAttributedStringCreate(NIL, string, dictionary) };
        let line = if attributed.is_null() {
            NIL
        } else {
            unsafe { CTLineCreateWithAttributedString(attributed) }
        };
        cf_release(attributed);
        cf_release(dictionary);
        cf_release(string);
        line
    }
    pub(super) fn cell(
        &mut self,
        context: Id,
        terminal: &zterm::Terminal,
        cell: &Cell,
        x: f64,
        y: f64,
        color: u32,
    ) {
        if cell.width == 0
            || cell.flags & HIDDEN != 0
            || cell.ch == '\u{10eeee}'
            || cell.ch == ' ' && cell.grapheme_id == 0 && cell.combining == ['\0'; 2]
        {
            return;
        }
        let style = u8::from(cell.flags & BOLD != 0) | (u8::from(cell.flags & ITALIC != 0) << 1);
        // Borrow the stack key on hits: full clusters do not allocate per paint.
        let mut bytes = [0u8; zterm::MAX_GRAPHEME_BYTES + 1];
        bytes[0] = b'0' + style;
        let mut n = 1;
        for ch in terminal.cell_chars(cell) {
            let length = ch.len_utf8();
            if n + length > bytes.len() {
                return;
            }
            ch.encode_utf8(&mut bytes[n..n + length]);
            n += length;
        }
        let key = core::str::from_utf8(&bytes[..n]).unwrap_or("");
        let line = if let Some(&line) = self.lines.get(key) {
            line
        } else {
            if self.lines.len() >= MAX_LINES {
                for (_, line) in core::mem::take(&mut self.lines) {
                    cf_release(line);
                }
            }
            let line = self.make_line(&key[1..], style);
            if !line.is_null() {
                self.lines.insert(String::from(key), line);
            }
            line
        };
        if !line.is_null() {
            unsafe {
                CGContextSetRGBFillColor(
                    context,
                    ((color >> 16) & 255) as f64 / 255.0,
                    ((color >> 8) & 255) as f64 / 255.0,
                    (color & 255) as f64 / 255.0,
                    1.0,
                );
                CGContextSetTextPosition(context, x, y + self.descent);
                CTLineDraw(line, context);
            }
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        for (_, line) in core::mem::take(&mut self.lines) {
            cf_release(line);
        }
        for font in self.fonts {
            cf_release(font);
        }
        unsafe { CGColorSpaceRelease(self.color_space) };
    }
}

impl Host {
    pub(super) fn draw(&mut self) {
        let context = send!(
            send!(class(c"NSGraphicsContext"), c"currentContext", Id),
            c"CGContext",
            Id
        );
        if context.is_null() {
            return;
        }
        let bounds = rect0(self.view, c"bounds");
        let padding = self.appearance.padding as f64;
        let width = self.renderer.cell_width;
        let height = self.renderer.cell_height;
        unsafe {
            CGContextSaveGState(context);
            CGContextClearRect(context, bounds);
            CGContextSetShouldAntialias(context, true);
        }
        fill(
            context,
            bounds,
            self.appearance.theme.background,
            self.appearance.opacity as f64 / 100.0,
        );
        let viewport = Rect::new(
            padding,
            bounds.size.height - padding - height * self.core.terminal.rows() as f64,
            width * self.core.terminal.cols() as f64,
            height * self.core.terminal.rows() as f64,
        );
        unsafe {
            CGContextClipToRect(context, viewport);
        }
        self.draw_images(context, bounds.size.height, i32::MIN, -1_073_741_824);
        for row in 0..self.core.terminal.rows() {
            for col in 0..self.core.terminal.cols() {
                let cell = self.core.terminal.view_cell(col, row, self.history);
                let selected = self.selection.is_some_and(|s| s.contains(row, col));
                if cell.flags & (EXPLICIT_BG | INVERSE) != 0 || selected {
                    let bg = if selected {
                        self.appearance.selection_background
                    } else if cell.flags & INVERSE != 0 {
                        cell.fg
                    } else {
                        cell.bg
                    };
                    fill(
                        context,
                        Rect::new(
                            padding + col as f64 * width,
                            bounds.size.height - padding - (row + 1) as f64 * height,
                            width,
                            height,
                        ),
                        bg,
                        1.0,
                    );
                }
            }
        }
        self.draw_images(context, bounds.size.height, -1_073_741_824, 0);
        for row in 0..self.core.terminal.rows() {
            for col in 0..self.core.terminal.cols() {
                let cell = self.core.terminal.view_cell(col, row, self.history);
                let selected = self.selection.is_some_and(|s| s.contains(row, col));
                let mut fg = if selected {
                    self.appearance.selection_foreground
                } else if cell.flags & INVERSE != 0 {
                    cell.bg
                } else {
                    cell.fg
                };
                if cell.flags & DIM != 0 {
                    fg = ((fg & 0xfefefe) >> 1)
                        + ((self.appearance.theme.background & 0xfefefe) >> 1);
                }
                let x = padding + col as f64 * width;
                let y = bounds.size.height - padding - (row + 1) as f64 * height;
                self.renderer
                    .cell(context, &self.core.terminal, cell, x, y, fg);
                if cell.flags & UNDERLINE != 0 {
                    fill(
                        context,
                        Rect::new(x, y + 1.0, width * cell.width.max(1) as f64, 1.0),
                        fg,
                        1.0,
                    );
                }
                if cell.flags & STRIKE != 0 {
                    fill(
                        context,
                        Rect::new(x, y + height * 0.45, width * cell.width.max(1) as f64, 1.0),
                        fg,
                        1.0,
                    );
                }
            }
        }
        self.draw_images(context, bounds.size.height, 0, i32::MAX);
        let (col, row) = self.core.terminal.cursor();
        let cx = padding + col as f64 * width;
        let cy = bounds.size.height - padding - (row + 1) as f64 * height;
        if !self.marked.is_empty() {
            let line = self.renderer.make_line(&self.marked, 0);
            fill(
                context,
                Rect::new(
                    cx,
                    cy,
                    (self.marked.chars().count() as f64 * width)
                        .min(viewport.size.width - col as f64 * width),
                    height,
                ),
                self.appearance.selection_background,
                1.0,
            );
            unsafe {
                let color = self.appearance.theme.foreground;
                CGContextSetRGBFillColor(
                    context,
                    ((color >> 16) & 255) as f64 / 255.0,
                    ((color >> 8) & 255) as f64 / 255.0,
                    (color & 255) as f64 / 255.0,
                    1.0,
                );
                CGContextSetTextPosition(context, cx, cy + self.renderer.descent);
                if !line.is_null() {
                    CTLineDraw(line, context);
                }
            }
            cf_release(line);
            fill(
                context,
                Rect::new(
                    cx,
                    cy + 1.0,
                    width * self.marked.chars().count() as f64,
                    1.0,
                ),
                self.appearance.cursor_color,
                1.0,
            );
        } else if self.history == 0
            && self.core.terminal.modes().cursor_visible
            && (self.blink_visible || !self.focused)
        {
            let shape = self
                .core
                .terminal
                .modes()
                .cursor_shape
                .unwrap_or(self.appearance.cursor_shape);
            let rect = match shape {
                CursorShape::Block => Rect::new(cx, cy, width, height),
                CursorShape::Underline => Rect::new(cx, cy, width, 2.0),
                CursorShape::Bar => Rect::new(cx, cy, 2.0, height),
            };
            fill(
                context,
                rect,
                self.appearance.cursor_color,
                if shape == CursorShape::Block {
                    0.5
                } else {
                    1.0
                },
            );
        }
        unsafe {
            CGContextRestoreGState(context);
        }
        self.core.terminal.clear_dirty();
    }
    pub(super) fn draw_images(&self, context: Id, view_height: f64, min_z: i32, max_z: i32) {
        unsafe {
            CGContextSetInterpolationQuality(context, 1);
        }
        for draw in self.core.terminal.graphics_draws(self.history) {
            if draw.z < min_z || (draw.z >= max_z && max_z != i32::MAX) {
                continue;
            }
            if draw.dest.width == 0
                || draw.dest.height == 0
                || draw.source.width == 0
                || draw.source.height == 0
            {
                continue;
            }
            // The provider owns an Arc reference, not a borrowed pixel pointer.
            // CoreGraphics may retain it beyond this callback; scene deletion
            // cannot invalidate its bytes. No second RGBA buffer is allocated.
            // We retain no image cache and release our native references here.
            unsafe {
                let image = draw.image;
                let owner = Arc::into_raw(Arc::clone(&image.pixels));
                let provider = CGDataProviderCreateWithData(
                    owner.cast_mut().cast(),
                    image.pixels.as_ptr().cast(),
                    image.pixels.len(),
                    Some(release_pixels),
                );
                if provider.is_null() {
                    drop(Arc::from_raw(owner));
                    continue;
                }
                let whole = CGImageCreate(
                    image.width as usize,
                    image.height as usize,
                    8,
                    32,
                    image.width as usize * 4,
                    self.renderer.color_space,
                    3 | (4 << 12),
                    provider,
                    ptr::null(),
                    false,
                    0,
                );
                if whole.is_null() {
                    CGDataProviderRelease(provider);
                    continue;
                }
                let crop = CGImageCreateWithImageInRect(
                    whole,
                    Rect::new(
                        draw.source.x as f64,
                        draw.source.y as f64,
                        draw.source.width as f64,
                        draw.source.height as f64,
                    ),
                );
                if !crop.is_null() {
                    let x = self.appearance.padding as f64 + draw.dest.x as f64 / self.scale;
                    let y = view_height
                        - self.appearance.padding as f64
                        - (draw.dest.y as f64 + draw.dest.height as f64) / self.scale;
                    CGContextSaveGState(context);
                    if let Some(clip) = draw.clip {
                        CGContextClipToRect(
                            context,
                            Rect::new(
                                self.appearance.padding as f64 + clip.x as f64 / self.scale,
                                view_height
                                    - self.appearance.padding as f64
                                    - (clip.y as f64 + clip.height as f64) / self.scale,
                                clip.width as f64 / self.scale,
                                clip.height as f64 / self.scale,
                            ),
                        );
                    }
                    CGContextDrawImage(
                        context,
                        Rect::new(
                            x,
                            y,
                            draw.dest.width as f64 / self.scale,
                            draw.dest.height as f64 / self.scale,
                        ),
                        crop,
                    );
                    CGContextRestoreGState(context);
                    CGImageRelease(crop);
                }
                CGImageRelease(whole);
                CGDataProviderRelease(provider);
            }
        }
    }
}

pub(super) fn fill(context: Id, rect: Rect, color: u32, alpha: f64) {
    unsafe {
        CGContextSetRGBFillColor(
            context,
            ((color >> 16) & 255) as f64 / 255.0,
            ((color >> 8) & 255) as f64 / 255.0,
            (color & 255) as f64 / 255.0,
            alpha,
        );
        CGContextFillRect(context, rect);
    }
}

// CoreGraphics invokes this only when the provider's final owner releases it.
// Arc permits that callback to run on a CoreGraphics worker thread safely.
pub(super) unsafe extern "C" fn release_pixels(info: Id, _: *const c_void, _: usize) {
    if !info.is_null() {
        unsafe {
            drop(Arc::from_raw(info.cast::<Vec<u8>>()));
        }
    }
}
