//! The X11 host owns the native window; the shared renderer owns EGL resources.
//! A failed GPU frame is discarded and the next full frame uses Xft.

use super::{Appearance, DrawnCursor, Selection, Window};
use crate::appearance::RendererKind;
use crate::gpu_worker::Renderer;
use alloc::{ffi::CString, format, string::String};
use core::ffi::{CStr, c_char, c_int, c_void};
use unicode_segmentation::UnicodeSegmentation;
use zterm::Terminal;
use ztty::gpu::{GridStyle, Rect, Style, grid::Painter};

#[link(name = "fontconfig")]
unsafe extern "C" {
    fn FcNameParse(name: *const u8) -> *mut c_void;
    fn FcNameUnparse(pattern: *mut c_void) -> *mut u8;
    fn FcPatternDestroy(pattern: *mut c_void);
    fn FcPatternGetDouble(
        pattern: *mut c_void,
        object: *const c_char,
        index: c_int,
        value: *mut f64,
    ) -> c_int;
    fn FcPatternDel(pattern: *mut c_void, object: *const c_char) -> c_int;
    fn FcPatternAddDouble(pattern: *mut c_void, object: *const c_char, value: f64) -> c_int;
    fn FcStrFree(value: *mut u8);
}

// Honor an explicit software environment before loading EGL drivers.
fn software_requested(kind: RendererKind) -> bool {
    kind == RendererKind::Software
        || (kind == RendererKind::Auto
            && crate::os::env(c"LIBGL_ALWAYS_SOFTWARE") == Some(&b"1"[..]))
}

fn software_driver(driver: &str) -> bool {
    let lower = driver.to_ascii_lowercase();
    [
        "llvmpipe",
        "softpipe",
        "lavapipe",
        "software rasterizer",
        "swrast",
    ]
    .iter()
    .any(|part| lower.contains(part))
}

fn rect(x: i32, y: i32, width: i32, height: i32) -> Rect {
    Rect {
        x: x as f32,
        y: y as f32,
        width: width.max(0) as f32,
        height: height.max(0) as f32,
    }
}

impl Window {
    fn gpu_font_pattern(&self) -> Result<String, &'static str> {
        // Xft resolves display DPI and X resources. Fontconfig's standalone
        // defaults can use a different DPI, even for the same point-size name.
        // Keep the original family/fallback request but make its pixel size
        // agree with the grid metrics that were validated by the X11 host.
        let name = CString::new(self.fonts.name.as_str()).map_err(|_| "invalid font name")?;
        unsafe {
            let mut pixels = 0.0f64;
            if FcPatternGetDouble((*self.font).pattern, c"pixelsize".as_ptr(), 0, &mut pixels) != 0
                || !pixels.is_finite()
                || !(4.0..=256.0).contains(&pixels)
            {
                return Err("invalid resolved font size");
            }
            let pattern = FcNameParse(name.as_ptr().cast());
            if pattern.is_null() {
                return Err("cannot copy the font pattern");
            }
            FcPatternDel(pattern, c"pixelsize".as_ptr());
            if FcPatternAddDouble(pattern, c"pixelsize".as_ptr(), pixels) == 0 {
                FcPatternDestroy(pattern);
                return Err("cannot set the resolved font size");
            }
            let encoded = FcNameUnparse(pattern);
            FcPatternDestroy(pattern);
            if encoded.is_null() {
                return Err("cannot encode the font pattern");
            }
            let result = CStr::from_ptr(encoded.cast())
                .to_str()
                .map(String::from)
                .map_err(|_| "invalid encoded font pattern");
            FcStrFree(encoded);
            result
        }
    }
    fn renderer_property(&self, name: &core::ffi::CStr, value: &str) {
        if self.state.id == 0 {
            return;
        }
        unsafe {
            let property = super::XInternAtom(self.state.display, name.as_ptr(), 0);
            super::XChangeProperty(
                self.state.display,
                self.state.id,
                property,
                self.utf8,
                8,
                0,
                value.as_ptr(),
                value.len() as i32,
            );
        }
    }

    pub(super) fn configure_gpu(&mut self, appearance: &Appearance) {
        self.retained_valid = false;
        self.pending_damage = None;
        self.renderer_kind = appearance.renderer;
        self.gpu_notice = None;
        if software_requested(appearance.renderer) {
            self.gpu = None;
            self.renderer_property(c"_ZTTY_GPU_WORKER", "");
            self.renderer_property(c"_ZTTY_RENDERER", "software");
            self.renderer_property(c"_ZTTY_GPU_DRIVER", "");
            return;
        }
        if appearance.renderer == RendererKind::Auto
            && self
                .gpu
                .as_ref()
                .is_some_and(|renderer| software_driver(renderer.driver()))
        {
            self.gpu = None;
            self.renderer_property(c"_ZTTY_GPU_WORKER", "");
            self.renderer_property(c"_ZTTY_RENDERER", "software");
            return;
        }
        if self.gpu.is_some() {
            let result = self
                .gpu_font_pattern()
                .and_then(|font| self.gpu.as_mut().unwrap().set_font(&font));
            if let Err(error) = result {
                self.gpu_failure(error);
            }
            return;
        }
        self.start_gpu();
    }

    fn start_gpu(&mut self) {
        if self.suspended
            || self.state.id == 0
            || self.gpu.is_some()
            || software_requested(self.renderer_kind)
        {
            return;
        }
        let result = (|| {
            let font = self.gpu_font_pattern()?;
            let visual = unsafe { super::XVisualIDFromVisual(self.visual) };
            unsafe {
                Renderer::new(
                    self.state.display.cast(),
                    self.state.id as usize,
                    visual as i32,
                    self.state.width as u32,
                    self.state.height as u32,
                    &font,
                )
            }
        })();
        match result {
            Ok(renderer) => {
                self.renderer_property(c"_ZTTY_GPU_DRIVER", renderer.driver());
                if self.renderer_kind == RendererKind::Auto && software_driver(renderer.driver()) {
                    self.renderer_property(c"_ZTTY_RENDERER", "software");
                    return;
                }
                self.renderer_property(c"_ZTTY_GPU_WORKER", &format!("{}", renderer.pid()));
                self.gpu = Some(renderer);
                self.resize_pixmap(1, 1);
                self.gpu_repaint = false;
                self.renderer_property(c"_ZTTY_RENDERER", "gpu");
            }
            Err(error) => {
                let report = self.renderer_kind == RendererKind::Gpu;
                self.gpu_failure(error);
                if !report {
                    self.gpu_notice = None;
                }
            }
        }
    }

    fn gpu_failure(&mut self, error: &'static str) {
        // Dropping the context before the X window also makes an ordinary Xft
        // redraw possible. Never publish the partially built fallback pixmap.
        self.gpu = None;
        self.renderer_kind = RendererKind::Software;
        self.renderer_property(c"_ZTTY_GPU_WORKER", "");
        self.renderer_property(c"_ZTTY_GPU_DRIVER", "");
        self.retained_valid = false;
        self.gpu_repaint = true;
        self.gpu_notice = Some(error);
        self.pending_damage = None;
        self.last_cursor = None;
        self.graphics_revision = 0;
        // A later overlay in this discarded GPU frame may already fall back
        // to Xft. Its drawable must be valid before the next full repaint.
        if !self.suspended {
            self.ensure_software_pixmap();
        }
        self.renderer_property(c"_ZTTY_RENDERER", "software");
    }

    pub(crate) fn renderer_failure_descriptor(&self) -> Option<crate::os::PollFd> {
        self.gpu.as_ref().map(Renderer::failure_descriptor)
    }

    pub fn take_renderer_notice(&mut self) -> Option<String> {
        if self.gpu.as_ref().is_some_and(Renderer::disconnected) {
            self.gpu_failure("GPU worker disconnected");
        }
        self.gpu_notice
            .take()
            .map(|error| format!("GPU renderer: {error}; using software rendering"))
    }

    pub fn needs_repaint(&self) -> bool {
        !self.suspended && (self.gpu_repaint || (self.gpu.is_none() && !self.retained_valid))
    }

    pub fn set_text_blink_visible(&mut self, visible: [bool; 2]) {
        self.text_blink_visible = visible;
    }

    pub(super) fn resize_gpu(&mut self) {
        if let Some(renderer) = &mut self.gpu
            && let Err(error) = renderer.resize(self.state.width as u32, self.state.height as u32)
        {
            self.gpu_failure(error);
        }
    }

    pub(super) fn visible_gpu(&mut self, visible: bool) {
        if visible {
            self.start_gpu();
        } else {
            // A hidden tab retains its terminal model, not a framebuffer and
            // glyph/image uploads. Its next visible frame rebuilds them once.
            self.gpu = None;
            self.renderer_property(c"_ZTTY_GPU_WORKER", "");
        }
    }

    pub(super) fn render_gpu(
        &mut self,
        terminal: &mut Terminal,
        offset: usize,
        selection: Option<Selection>,
        cursor: Option<DrawnCursor>,
    ) -> bool {
        let Some(renderer) = &mut self.gpu else {
            return false;
        };
        let cols = terminal.cols();
        let search_cols = self.search_cols;
        let search_bits = &self.search_bits;
        let selected = |col, row: usize| {
            let bit = row.saturating_mul(search_cols).saturating_add(col);
            selection.is_some_and(|s| s.contains(col, row, cols))
                || search_bits
                    .get(bit / 64)
                    .is_some_and(|word| word & (1 << (bit % 64)) != 0)
        };
        let style = GridStyle {
            padding: self.padding as f32,
            cell_width: self.state.cw as f32,
            cell_height: self.state.ch as f32,
            ascent: unsafe { (*self.font).ascent + 1 } as f32,
            background: self.background,
            background_alpha: if self.owns_colormap {
                (u16::from(self.requested_opacity) * 255 / 100) as u8
            } else {
                255
            },
            selection_foreground: self.selection_colors.0,
            selection_background: self.selection_colors.1,
            cursor,
            cursor_color: if self.state.focused {
                self.cursor_color
            } else {
                ((self.cursor_color & 0xfefefe) >> 1) + ((self.background & 0xfefefe) >> 1)
            },
            slow_blink_visible: self.text_blink_visible[0],
            rapid_blink_visible: self.text_blink_visible[1],
        };
        if let Err(error) = renderer.draw_terminal(terminal, offset, style, selected) {
            self.gpu_failure(error);
            return false;
        }
        self.last_cursor = cursor;
        self.graphics_revision = terminal.graphics().revision();
        self.graphics_alternate = terminal.modes().alternate_screen;
        terminal.clear_dirty();
        self.damage(0, self.state.height);
        true
    }

    pub(super) fn finish_gpu_frame(&mut self) -> bool {
        if self.gpu_repaint {
            self.pending_damage = None;
            return true;
        }
        let Some(renderer) = &mut self.gpu else {
            return false;
        };
        if self.pending_damage.take().is_some() && !self.suspended && self.state.id != 0 {
            match renderer.present() {
                Ok(()) => self.reveal_pending = false,
                Err(error) => self.gpu_failure(error),
            }
        }
        true
    }

    pub(super) fn gpu_fill(&mut self, rgb: u32, x: i32, y: i32, w: u32, h: u32) -> bool {
        let Some(renderer) = &mut self.gpu else {
            return false;
        };
        let rectangle = rect(
            x,
            y,
            w.min(i32::MAX as u32) as i32,
            h.min(i32::MAX as u32) as i32,
        );
        if let Err(error) = renderer.draw_rect(rectangle, rgb, 255) {
            self.gpu_failure(error);
        }
        true
    }

    pub(super) fn gpu_overlay_width(&mut self, text: &str) -> Option<i32> {
        let renderer = self.gpu.as_mut()?;
        match renderer.measure_text(text, Style::default()) {
            Ok(width) => Some((width + 0.999).max(0.0) as i32),
            Err(error) => {
                self.gpu_failure(error);
                None
            }
        }
    }

    pub(super) fn gpu_text(&mut self, text: &str, bounds: crate::layout::Rect, rgb: u32) -> bool {
        let Some(renderer) = &mut self.gpu else {
            return false;
        };
        let result = (|| {
            renderer.clip(Some(rect(bounds.x, bounds.y, bounds.width, bounds.height)))?;
            let baseline = bounds.y + unsafe { (*self.font).ascent } + 1;
            let rows = (bounds.height / self.state.ch).clamp(0, 32) as usize;
            for (row, line) in text.split('\n').take(rows).enumerate() {
                // Scene strings are already bounded. Menus also use this path;
                // never let an arbitrarily large label bypass the shaper limit.
                let end = line
                    .grapheme_indices(true)
                    .map(|(start, grapheme)| start + grapheme.len())
                    .take_while(|end| *end <= 16_384)
                    .last()
                    .unwrap_or(0);
                renderer.draw_text(
                    &line[..end],
                    bounds.x as f32,
                    (baseline + row as i32 * self.state.ch) as f32,
                    rgb,
                    Style::default(),
                )?;
            }
            renderer.clip(None)
        })();
        if let Err(error) = result {
            self.gpu_failure(error);
        }
        true
    }

    pub(super) fn gpu_native_image(
        &mut self,
        image_id: u32,
        image: &zterm::graphics::Image,
        bounds: crate::layout::Rect,
    ) -> bool {
        let Some(renderer) = &mut self.gpu else {
            return false;
        };
        let result = renderer.draw_image(
            u64::from(image_id),
            image.generation,
            (image.width, image.height),
            &image.pixels,
            Rect {
                x: 0.0,
                y: 0.0,
                width: image.width as f32,
                height: image.height as f32,
            },
            rect(bounds.x, bounds.y, bounds.width, bounds.height),
        );
        if let Err(error) = result {
            self.gpu_failure(error);
        }
        true
    }
}
