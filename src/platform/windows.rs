//! Native Win32 frontend. GDI draws text into a retained DIB; the image path
//! composites decoded RGBA pixels itself. ConPTY pipe work lives in zpty.

use crate::{Config, appearance::Appearance, os};
use alloc::{boxed::Box, string::String, vec::Vec};
use core::{
    cell::{Cell, UnsafeCell},
    ffi::c_void,
    ptr,
};
use zterm::{
    CursorShape, Terminal,
    input::{self, Key, KeyEvent, ModifierKey, Modifiers, MouseEvent},
};

use ztty::session_core::{ReadStep, SessionCore};

type Handle = *mut c_void;
const NULL: Handle = ptr::null_mut();
const WINDOW_STYLE: u32 = 0x00cf0000;
const MAX_INPUT: usize = 8 * 1024 * 1024;
const GWLP_USERDATA: i32 = -21;
const WM_PAINT: u32 = 0x000f;
const WM_CLOSE: u32 = 0x0010;
const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_CHAR: u32 = 0x0102;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_SYSKEYUP: u32 = 0x0105;
const WM_SYSCHAR: u32 = 0x0106;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}
fn colorref(rgb: u32) -> u32 {
    (rgb & 0xff00) | ((rgb >> 16) & 0xff) | ((rgb & 0xff) << 16)
}

struct Surface {
    dc: Handle,
    bitmap: Handle,
    previous: Handle,
    pixels: *mut u32,
    width: i32,
    height: i32,
}
impl Surface {
    fn new() -> Result<Self, &'static str> {
        let dc = unsafe { ffi::CreateCompatibleDC(NULL) };
        if dc.is_null() {
            return Err("cannot create a Windows drawing context");
        }
        Ok(Self {
            dc,
            bitmap: NULL,
            previous: NULL,
            pixels: ptr::null_mut(),
            width: 0,
            height: 0,
        })
    }
    fn resize(&mut self, width: i32, height: i32) -> bool {
        if width == self.width && height == self.height {
            return true;
        }
        if width <= 0 || height <= 0 || (width as u64) * (height as u64) > 16 * 1024 * 1024 {
            return false;
        }
        let info = ffi::BitmapInfo {
            header: ffi::BitmapInfoHeader {
                size: 40,
                width,
                height: -height,
                planes: 1,
                bits: 32,
                compression: 0,
                image_size: 0,
                x_ppm: 0,
                y_ppm: 0,
                colors: 0,
                important: 0,
            },
            palette: [0],
        };
        let mut pixels = ptr::null_mut();
        let bitmap = unsafe { ffi::CreateDIBSection(self.dc, &info, 0, &mut pixels, NULL, 0) };
        if bitmap.is_null() {
            return false;
        }
        let previous = unsafe { ffi::SelectObject(self.dc, bitmap) };
        if self.previous.is_null() {
            self.previous = previous;
        }
        if !self.bitmap.is_null() {
            unsafe {
                ffi::DeleteObject(self.bitmap);
            }
        }
        self.bitmap = bitmap;
        self.pixels = pixels.cast();
        self.width = width;
        self.height = height;
        self.fill(0, 0, width, height, 0);
        true
    }
    fn fill(&mut self, x: i32, y: i32, width: i32, height: i32, rgb: u32) {
        if self.pixels.is_null() {
            return;
        }
        let left = x.clamp(0, self.width);
        let right = x.saturating_add(width).clamp(0, self.width);
        let top = y.clamp(0, self.height);
        let bottom = y.saturating_add(height).clamp(0, self.height);
        if left >= right || top >= bottom {
            return;
        }
        for row in top..bottom {
            unsafe {
                core::slice::from_raw_parts_mut(
                    self.pixels.add((row * self.width + left) as usize),
                    (right - left) as usize,
                )
            }
            .fill(rgb & 0xffffff);
        }
    }
    fn image(
        &mut self,
        draw: &zterm::graphics::Draw<'_>,
        padding: i32,
        cols: usize,
        rows: usize,
        cell_width: i32,
        cell_height: i32,
    ) {
        if self.pixels.is_null()
            || draw.dest.width == 0
            || draw.dest.height == 0
            || draw.source.width == 0
            || draw.source.height == 0
        {
            return;
        }
        let dx = i64::from(draw.dest.x) + i64::from(padding);
        let dy = i64::from(draw.dest.y) + i64::from(padding);
        let left = dx.max(padding as i64).max(0);
        let top = dy.max(padding as i64).max(0);
        let right = (dx + i64::from(draw.dest.width))
            .min(i64::from(padding) + cols as i64 * i64::from(cell_width))
            .min(self.width as i64);
        let bottom = (dy + i64::from(draw.dest.height))
            .min(i64::from(padding) + rows as i64 * i64::from(cell_height))
            .min(self.height as i64);
        if left >= right || top >= bottom {
            return;
        }
        let image = draw.image;
        for y in top..bottom {
            let sy = u64::from(draw.source.y)
                + (y - dy) as u64 * u64::from(draw.source.height) / u64::from(draw.dest.height);
            if sy >= u64::from(image.height) {
                continue;
            }
            for x in left..right {
                let sx = u64::from(draw.source.x)
                    + (x - dx) as u64 * u64::from(draw.source.width) / u64::from(draw.dest.width);
                if sx >= u64::from(image.width) {
                    continue;
                }
                let source = ((sy * u64::from(image.width) + sx) * 4) as usize;
                let Some(rgba) = image.pixels.get(source..source + 4) else {
                    continue;
                };
                let alpha = u32::from(rgba[3]);
                if alpha == 0 {
                    continue;
                }
                let target = unsafe { &mut *self.pixels.add((y * self.width as i64 + x) as usize) };
                let red =
                    (u32::from(rgba[0]) * alpha + ((*target >> 16) & 255) * (255 - alpha) + 127)
                        / 255;
                let green =
                    (u32::from(rgba[1]) * alpha + ((*target >> 8) & 255) * (255 - alpha) + 127)
                        / 255;
                let blue =
                    (u32::from(rgba[2]) * alpha + (*target & 255) * (255 - alpha) + 127) / 255;
                *target = (red << 16) | (green << 8) | blue;
            }
        }
    }
}
impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            if !self.previous.is_null() {
                ffi::SelectObject(self.dc, self.previous);
            }
            if !self.bitmap.is_null() {
                ffi::DeleteObject(self.bitmap);
            }
            ffi::DeleteDC(self.dc);
        }
    }
}

struct Fonts {
    normal: Handle,
    bold: Handle,
    italic: Handle,
    bold_italic: Handle,
    width: i32,
    height: i32,
}
impl Fonts {
    fn new(dc: Handle, description: &str, dpi: u32) -> Result<Self, &'static str> {
        let (family, properties) = description.split_once(':').unwrap_or((description, ""));
        let size = properties
            .split(':')
            .find_map(|item| item.strip_prefix("size="))
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(11)
            .clamp(5, 96);
        let family = wide(if family.is_empty() || family == "monospace" {
            "Consolas"
        } else {
            family
        });
        let height = -(((size * dpi.max(96) + 36) / 72) as i32);
        let make = |weight, italic| unsafe {
            ffi::CreateFontW(
                height,
                0,
                0,
                0,
                weight,
                italic,
                0,
                0,
                1,
                0,
                0,
                5,
                0x31,
                family.as_ptr(),
            )
        };
        let mut result = Self {
            normal: make(400, 0),
            bold: make(700, 0),
            italic: make(400, 1),
            bold_italic: make(700, 1),
            width: 0,
            height: 0,
        };
        if result.normal.is_null()
            || result.bold.is_null()
            || result.italic.is_null()
            || result.bold_italic.is_null()
        {
            return Err("cannot create the requested Windows font");
        }
        let old = unsafe { ffi::SelectObject(dc, result.normal) };
        let mut metrics: ffi::TextMetric = unsafe { core::mem::zeroed() };
        let success = unsafe { ffi::GetTextMetricsW(dc, &mut metrics) };
        unsafe {
            ffi::SelectObject(dc, old);
        }
        if success == 0 {
            return Err("cannot measure the requested Windows font");
        }
        result.width = metrics.average_width.max(1);
        result.height = (metrics.height + metrics.external_leading).max(1);
        Ok(result)
    }
    fn face(&self, flags: u16) -> Handle {
        match (flags & zterm::BOLD != 0, flags & zterm::ITALIC != 0) {
            (true, true) => self.bold_italic,
            (true, false) => self.bold,
            (false, true) => self.italic,
            _ => self.normal,
        }
    }
}
impl Drop for Fonts {
    fn drop(&mut self) {
        for face in [self.normal, self.bold, self.italic, self.bold_italic] {
            if !face.is_null() {
                unsafe {
                    ffi::DeleteObject(face);
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Selection {
    start: (usize, usize),
    end: (usize, usize),
}
impl Selection {
    fn contains(self, col: usize, row: usize, cols: usize) -> bool {
        let a = self.start.1 * cols + self.start.0;
        let b = self.end.1 * cols + self.end.0;
        (a.min(b)..=a.max(b)).contains(&(row * cols + col))
    }
    fn text(self, terminal: &Terminal, history: usize) -> String {
        let cols = terminal.cols();
        let mut output = String::new();
        let a = self.start.1 * cols + self.start.0;
        let b = self.end.1 * cols + self.end.0;
        for row in a.min(b) / cols..=(a.max(b) / cols).min(terminal.rows() - 1) {
            let start = if row == a.min(b) / cols {
                a.min(b) % cols
            } else {
                0
            };
            let end = if row == a.max(b) / cols {
                a.max(b) % cols + 1
            } else {
                cols
            };
            let line_start = output.len();
            for col in start..end {
                let cell = terminal.view_cell(col, row, history);
                if cell.width == 0 {
                    continue;
                }
                output.push(cell.ch);
                for ch in cell.combining {
                    if ch != '\0' {
                        output.push(ch);
                    }
                }
            }
            while output.len() > line_start && output.ends_with(' ') {
                output.pop();
            }
            if row != a.max(b) / cols {
                output.push('\n');
            }
        }
        output
    }
}

#[derive(Clone, Copy)]
struct PendingKey {
    key: Key,
    modifiers: Modifiers,
    event: KeyEvent,
}

struct App {
    hwnd: Handle,
    core: SessionCore,
    appearance: Appearance,
    surface: Surface,
    fonts: Fonts,
    encoded: Vec<u8>,
    running: bool,
    dirty: bool,
    history: usize,
    selection: Option<Selection>,
    dragging: bool,
    held: [Option<Key>; 256],
    intercepted: [bool; 256],
    pending_key: Option<PendingKey>,
    suppress_char: bool,
    surrogate: u16,
    title: String,
    graphics_revision: u64,
    sync_since: Option<u64>,
    cursor_on: bool,
    blink_deadline: u64,
    last_cursor: (usize, usize),
    focused: bool,
}

impl App {
    fn enqueue(&mut self, bytes: &[u8]) {
        if !self.core.input.push(bytes) {
            os::print(2, b"ztty: input queue full; paste was rejected\n");
        }
    }
    fn enqueue_encoded(&mut self) {
        let encoded = core::mem::take(&mut self.encoded);
        self.enqueue(&encoded);
        self.encoded = encoded;
        self.encoded.clear();
    }
    fn flush(&mut self) {
        // The event loop still owns process lifetime and readiness policy.
        // A failed write keeps its unwritten suffix in the shared queue.
        let _ = self.core.flush_input();
    }
    fn input(&mut self, key: Key, modifiers: Modifiers, event: KeyEvent, text: &str) {
        self.encoded.clear();
        input::encode_key_into(
            key,
            modifiers,
            event,
            text,
            &self.core.terminal.modes().input(),
            &mut self.encoded,
        );
        self.enqueue_encoded();
        self.history = 0;
        self.selection = None;
        self.reset_cursor();
    }
    fn reset_cursor(&mut self) {
        self.cursor_on = true;
        self.blink_deadline = os::milliseconds() + self.appearance.cursor_blink_interval;
        self.dirty = true;
    }
    fn cell_at(&self, x: i32, y: i32) -> (usize, usize) {
        (
            (((x - self.appearance.padding).max(0) / self.fonts.width) as usize)
                .min(self.core.terminal.cols() - 1),
            (((y - self.appearance.padding).max(0) / self.fonts.height) as usize)
                .min(self.core.terminal.rows() - 1),
        )
    }
    fn resize(&mut self) {
        self.resize_with_fonts(None);
    }
    fn resize_with_fonts(&mut self, candidate: Option<Fonts>) {
        let mut rect = ffi::Rect::default();
        unsafe {
            ffi::GetClientRect(self.hwnd, &mut rect);
        }
        if !self.surface.resize(rect.right.max(1), rect.bottom.max(1)) {
            return;
        }
        let fonts = candidate.as_ref().unwrap_or(&self.fonts);
        let cols = ((rect.right - 2 * self.appearance.padding).max(fonts.width) / fonts.width)
            .clamp(1, 1024) as usize;
        let rows = ((rect.bottom - 2 * self.appearance.padding).max(fonts.height) / fonts.height)
            .clamp(1, 1024) as usize;
        let size = zpty::Size {
            cols: cols as u16,
            rows: rows as u16,
            pixel_width: (cols as i32 * fonts.width).min(u16::MAX as i32) as u16,
            pixel_height: (rows as i32 * fonts.height).min(u16::MAX as i32) as u16,
        };
        if self
            .core
            .resize_with_cell_size(size, (fonts.width as u32, fonts.height as u32))
            .is_err()
        {
            os::print(2, b"ztty: PTY resize failed; retaining the terminal grid\n");
            self.dirty = true;
            unsafe {
                ffi::InvalidateRect(self.hwnd, ptr::null(), 0);
            }
            return;
        }
        if let Some(fonts) = candidate {
            self.fonts = fonts;
        }
        self.selection = None;
        self.history = self.history.min(self.core.terminal.history_len());
        self.dirty = true;
        unsafe {
            ffi::InvalidateRect(self.hwnd, ptr::null(), 0);
        }
    }
    fn tick(&mut self) {
        self.flush();
        let mut buffer = [0u8; 65536];
        for _ in 0..16 {
            match self.core.read_once(&mut buffer, false) {
                Ok(ReadStep::Data {
                    history_before,
                    history_after,
                    ..
                }) => {
                    if self.history != 0 {
                        self.history = self
                            .history
                            .saturating_add(history_after.saturating_sub(history_before))
                            .min(history_after);
                    }
                    self.dirty = true;
                }
                Ok(ReadStep::Pending) => break,
                Ok(ReadStep::Eof) | Err(_) => {
                    self.running = false;
                    break;
                }
            }
        }
        // Preserve the existing ConPTY exit policy. Reaping alone does not
        // imply pipe EOF; waiting for it may require closing the pseudoconsole.
        if self.core.pty.reap().ok().flatten().is_some() {
            self.running = false;
        }
        if !self.core.queue_responses() {
            os::print(
                2,
                b"ztty: input queue full; terminal replies were rejected\n",
            );
        }
        self.flush();
        if self.title != self.core.terminal.title() {
            self.title = String::from(self.core.terminal.title());
            let title = wide(if self.title.is_empty() {
                "ztty"
            } else {
                &self.title
            });
            unsafe {
                ffi::SetWindowTextW(self.hwnd, title.as_ptr());
            }
        }
        let now = os::milliseconds();
        let cursor = self.core.terminal.cursor();
        if cursor != self.last_cursor {
            self.last_cursor = cursor;
            self.reset_cursor();
            self.ime_position();
        }
        if self.core.terminal.modes().synchronized_output {
            let start = *self.sync_since.get_or_insert(now);
            if now.saturating_sub(start) < 150 {
                return;
            }
            self.core.terminal.force_end_sync();
        }
        self.sync_since = None;
        if self
            .core
            .terminal
            .modes()
            .cursor_blink
            .unwrap_or(self.appearance.cursor_blink)
            && self.focused
            && now >= self.blink_deadline
        {
            self.cursor_on = !self.cursor_on;
            self.blink_deadline = now + self.appearance.cursor_blink_interval;
            self.dirty = true;
        }
        let revision = self.core.terminal.graphics().revision();
        if revision != self.graphics_revision {
            self.graphics_revision = revision;
            self.dirty = true;
        }
        if self.dirty || self.core.terminal.dirty() {
            unsafe {
                ffi::InvalidateRect(self.hwnd, ptr::null(), 0);
            }
        }
    }
    fn images(&mut self, range: core::ops::Range<i64>) {
        for draw in self.core.terminal.graphics_draws(self.history) {
            if range.contains(&i64::from(draw.z)) {
                self.surface.image(
                    &draw,
                    self.appearance.padding,
                    self.core.terminal.cols(),
                    self.core.terminal.rows(),
                    self.fonts.width,
                    self.fonts.height,
                );
            }
        }
    }
    fn draw(&mut self) {
        if self.surface.pixels.is_null() {
            return;
        }
        // GDI batches drawing. Flush before touching its bitmap from Rust.
        unsafe {
            ffi::GdiFlush();
        }
        let p = self.appearance.padding;
        let cw = self.fonts.width;
        let ch = self.fonts.height;
        self.surface.fill(
            0,
            0,
            self.surface.width,
            self.surface.height,
            self.appearance.theme.background,
        );
        for row in 0..self.core.terminal.rows() {
            for col in 0..self.core.terminal.cols() {
                let cell = self.core.terminal.view_cell(col, row, self.history);
                let bg = if cell.flags & zterm::INVERSE != 0 {
                    cell.fg
                } else {
                    cell.bg
                };
                self.surface
                    .fill(p + col as i32 * cw, p + row as i32 * ch, cw, ch, bg);
            }
        }
        self.images(i32::MIN as i64..-1073741824);
        for row in 0..self.core.terminal.rows() {
            for col in 0..self.core.terminal.cols() {
                let cell = self.core.terminal.view_cell(col, row, self.history);
                if cell.flags & (zterm::EXPLICIT_BG | zterm::INVERSE) != 0 {
                    self.surface.fill(
                        p + col as i32 * cw,
                        p + row as i32 * ch,
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
        self.images(-1073741824..0);
        for row in 0..self.core.terminal.rows() {
            for col in 0..self.core.terminal.cols() {
                if self
                    .selection
                    .is_some_and(|s| s.contains(col, row, self.core.terminal.cols()))
                {
                    self.surface.fill(
                        p + col as i32 * cw,
                        p + row as i32 * ch,
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
        for row in 0..self.core.terminal.rows() {
            for col in 0..self.core.terminal.cols() {
                let cell = self.core.terminal.view_cell(col, row, self.history);
                if cell.width == 0 || cell.flags & zterm::HIDDEN != 0 {
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
                if self
                    .selection
                    .is_some_and(|s| s.contains(col, row, self.core.terminal.cols()))
                {
                    fg = self.appearance.selection_foreground;
                }
                let x = p + col as i32 * cw;
                let y = p + row as i32 * ch;
                let clip = ffi::Rect {
                    left: x,
                    top: y,
                    right: (x + cw * cell.width as i32)
                        .min(p + self.core.terminal.cols() as i32 * cw),
                    bottom: y + ch,
                };
                let mut text = [0u16; 6];
                let mut length = 0;
                for scalar in core::iter::once(cell.ch)
                    .chain(cell.combining)
                    .filter(|&c| c != '\0')
                {
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
        for row in 0..self.core.terminal.rows() {
            for col in 0..self.core.terminal.cols() {
                let cell = self.core.terminal.view_cell(col, row, self.history);
                if cell.width == 0 || cell.flags & zterm::HIDDEN != 0 {
                    continue;
                }
                let fg = if cell.flags & zterm::INVERSE != 0 {
                    cell.bg
                } else {
                    cell.fg
                };
                let x = p + col as i32 * cw;
                let y = p + row as i32 * ch;
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
        self.images(0..i32::MAX as i64 + 1);
        if self.history == 0
            && self.core.terminal.modes().cursor_visible
            && (self.cursor_on
                || !self.focused
                || !self
                    .core
                    .terminal
                    .modes()
                    .cursor_blink
                    .unwrap_or(self.appearance.cursor_blink))
        {
            let (col, row) = self.core.terminal.cursor();
            let x = p + col as i32 * cw;
            let y = p + row as i32 * ch;
            let color = self.appearance.cursor_color;
            match self
                .core
                .terminal
                .modes()
                .cursor_shape
                .unwrap_or(self.appearance.cursor_shape)
            {
                CursorShape::Underline => self.surface.fill(x, y + ch - 2, cw, 2, color),
                CursorShape::Bar => self.surface.fill(x, y, 2, ch, color),
                CursorShape::Block => {
                    if !self.focused {
                        self.surface.fill(x, y, cw, 1, color);
                        self.surface.fill(x, y + ch - 1, cw, 1, color);
                        self.surface.fill(x, y, 1, ch, color);
                        self.surface.fill(x + cw - 1, y, 1, ch, color);
                    } else {
                        let cell = self.core.terminal.cell(col, row);
                        let width = cw * i32::from(cell.width.max(1));
                        self.surface.fill(x, y, width, ch, color);
                        if cell.width != 0 && cell.flags & zterm::HIDDEN == 0 {
                            let mut text = [0u16; 6];
                            let mut length = 0;
                            for scalar in core::iter::once(cell.ch)
                                .chain(cell.combining)
                                .filter(|&c| c != '\0')
                            {
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
                                right: (x + width).min(p + self.core.terminal.cols() as i32 * cw),
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
        self.dirty = false;
        self.core.terminal.clear_dirty();
    }
    fn paint(&mut self) {
        let mut paint: ffi::PaintStruct = unsafe { core::mem::zeroed() };
        let dc = unsafe { ffi::BeginPaint(self.hwnd, &mut paint) };
        if !self.core.terminal.modes().synchronized_output
            && (self.dirty || self.core.terminal.dirty())
        {
            self.draw();
        }
        if !self.surface.bitmap.is_null() {
            unsafe {
                ffi::BitBlt(
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
        unsafe {
            ffi::EndPaint(self.hwnd, &paint);
        }
    }
    fn ime_position(&self) {
        let context = unsafe { ffi::ImmGetContext(self.hwnd) };
        if context.is_null() {
            return;
        }
        let (col, row) = self.core.terminal.cursor();
        let form = ffi::CompositionForm {
            style: 2,
            position: ffi::Point {
                x: self.appearance.padding + col as i32 * self.fonts.width,
                y: self.appearance.padding + (row + 1) as i32 * self.fonts.height,
            },
            area: ffi::Rect::default(),
        };
        unsafe {
            ffi::ImmSetCompositionWindow(context, &form);
            ffi::ImmReleaseContext(self.hwnd, context);
        }
    }
    fn copy(&self) {
        let Some(selection) = self.selection else {
            return;
        };
        let text = wide(&selection.text(&self.core.terminal, self.history));
        if unsafe { ffi::OpenClipboard(self.hwnd) } == 0 {
            return;
        }
        let memory = unsafe { ffi::GlobalAlloc(2, text.len() * 2) };
        if !memory.is_null() {
            let target = unsafe { ffi::GlobalLock(memory) }.cast::<u16>();
            if !target.is_null() {
                unsafe {
                    ptr::copy_nonoverlapping(text.as_ptr(), target, text.len());
                    ffi::GlobalUnlock(memory);
                    ffi::EmptyClipboard();
                }
                if unsafe { ffi::SetClipboardData(13, memory) }.is_null() {
                    unsafe {
                        ffi::GlobalFree(memory);
                    }
                }
            } else {
                unsafe {
                    ffi::GlobalFree(memory);
                }
            }
        }
        unsafe {
            ffi::CloseClipboard();
        }
    }
    fn paste(&mut self) {
        if unsafe { ffi::OpenClipboard(self.hwnd) } == 0 {
            return;
        }
        let memory = unsafe { ffi::GetClipboardData(13) };
        if !memory.is_null() {
            let size = unsafe { ffi::GlobalSize(memory) };
            if size != 0 && size <= 16 * 1024 * 1024 {
                let text = unsafe { ffi::GlobalLock(memory) }.cast::<u16>();
                if !text.is_null() {
                    let units = unsafe { core::slice::from_raw_parts(text, size / 2) };
                    let length = units.iter().position(|&c| c == 0).unwrap_or(units.len());
                    let text = String::from_utf16_lossy(&units[..length]);
                    self.encoded.clear();
                    input::encode_paste_into(
                        &text,
                        &self.core.terminal.modes().input(),
                        &mut self.encoded,
                    );
                    self.enqueue_encoded();
                    unsafe {
                        ffi::GlobalUnlock(memory);
                    }
                }
            }
        }
        unsafe {
            ffi::CloseClipboard();
        }
        self.history = 0;
        self.reset_cursor();
    }
    fn key(&mut self, message: u32, vk: usize, flags: isize) -> bool {
        if vk >= 256 {
            return false;
        }
        let pressed = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
        let mut mods = modifiers();
        // Windows represents AltGr as synthetic Ctrl + right Alt. Let the
        // keyboard layout commit its text instead of encoding a Ctrl shortcut.
        if unsafe { ffi::GetKeyState(0xa5) } < 0 && mods.contains(Modifiers::CTRL) {
            mods.0 &= !(Modifiers::CTRL | Modifiers::ALT);
        }
        if pressed && vk == 0x73 && mods.contains(Modifiers::ALT) {
            return false;
        }
        if pressed {
            self.suppress_char = false;
            self.pending_key = None;
            if mods.contains(Modifiers::CTRL)
                && mods.contains(Modifiers::SHIFT)
                && (vk == b'C' as usize || vk == b'V' as usize)
            {
                if vk == b'C' as usize {
                    self.copy();
                } else {
                    self.paste();
                }
                self.intercepted[vk] = true;
                self.suppress_char = true;
                return true;
            }
            if mods.contains(Modifiers::SHIFT) && (vk == 0x21 || vk == 0x22) {
                let step = self.core.terminal.rows().saturating_sub(1).max(1);
                self.history = if vk == 0x21 {
                    self.history
                        .saturating_add(step)
                        .min(self.core.terminal.history_len())
                } else {
                    self.history.saturating_sub(step)
                };
                self.selection = None;
                self.dirty = true;
                self.intercepted[vk] = true;
                self.suppress_char = true;
                return true;
            }
        } else if self.intercepted[vk] {
            self.intercepted[vk] = false;
            self.held[vk] = None;
            return true;
        }
        let key = if !pressed {
            self.held[vk].unwrap_or_else(|| virtual_key(vk as u32, flags))
        } else {
            virtual_key(vk as u32, flags)
        };
        self.held[vk] = if pressed { Some(key) } else { None };
        let event = if !pressed {
            KeyEvent::Release
        } else if flags & (1 << 30) != 0 {
            KeyEvent::Repeat
        } else {
            KeyEvent::Press
        };
        if !pressed
            || !matches!(key, Key::Character(_) | Key::Unidentified)
            || mods.contains(Modifiers::CTRL)
        {
            self.input(key, mods, event, "");
            if pressed {
                self.suppress_char = true;
            }
        } else {
            self.pending_key = Some(PendingKey {
                key,
                modifiers: mods,
                event,
            });
        }
        true
    }
    fn character(&mut self, unit: u16) {
        if self.suppress_char {
            return;
        }
        if (0xd800..=0xdbff).contains(&unit) {
            self.surrogate = unit;
            return;
        }
        let scalar = if (0xdc00..=0xdfff).contains(&unit) && self.surrogate != 0 {
            let scalar =
                0x10000 + (((self.surrogate as u32 - 0xd800) << 10) | (unit as u32 - 0xdc00));
            self.surrogate = 0;
            char::from_u32(scalar).unwrap_or('\u{fffd}')
        } else {
            self.surrogate = 0;
            char::from_u32(unit as u32).unwrap_or('\u{fffd}')
        };
        let mut buffer = [0u8; 4];
        let text = scalar.encode_utf8(&mut buffer);
        if let Some(key) = self.pending_key.take() {
            self.input(key.key, key.modifiers, key.event, text);
        } else {
            self.encoded.clear();
            input::encode_text_into(text, &self.core.terminal.modes().input(), &mut self.encoded);
            self.enqueue_encoded();
            self.history = 0;
            self.reset_cursor();
        }
    }
    fn mouse(&mut self, message: u32, wparam: usize, lparam: isize) {
        let mut point = ffi::Point {
            x: lparam as i16 as i32,
            y: (lparam >> 16) as i16 as i32,
        };
        if message == 0x020a {
            unsafe {
                ffi::ScreenToClient(self.hwnd, &mut point);
            }
        }
        let (col, row) = self.cell_at(point.x, point.y);
        let mods = modifiers();
        let app_tracking =
            self.core.terminal.modes().mouse_tracking != 0 && !mods.contains(Modifiers::SHIFT);
        let event = match message {
            0x0201 => Some(MouseEvent::Press {
                button: 0,
                col,
                row,
            }),
            0x0202 => Some(MouseEvent::Release {
                button: 0,
                col,
                row,
            }),
            0x0204 => Some(MouseEvent::Press {
                button: 2,
                col,
                row,
            }),
            0x0205 => Some(MouseEvent::Release {
                button: 2,
                col,
                row,
            }),
            0x0207 => Some(MouseEvent::Press {
                button: 1,
                col,
                row,
            }),
            0x0208 => Some(MouseEvent::Release {
                button: 1,
                col,
                row,
            }),
            0x0200 => Some(MouseEvent::Motion {
                button: if wparam & 1 != 0 {
                    Some(0)
                } else if wparam & 0x10 != 0 {
                    Some(1)
                } else if wparam & 2 != 0 {
                    Some(2)
                } else {
                    None
                },
                col,
                row,
            }),
            0x020a => Some(MouseEvent::Scroll {
                up: (wparam >> 16) as i16 > 0,
                col,
                row,
            }),
            _ => None,
        };
        if app_tracking {
            if let Some(event) = event {
                self.encoded.clear();
                input::encode_mouse_into(
                    event,
                    mods,
                    &self.core.terminal.modes().input(),
                    &mut self.encoded,
                );
                self.enqueue_encoded();
            }
            return;
        }
        match message {
            0x0201 => {
                if mods.contains(Modifiers::CTRL) {
                    self.open_link(col, row);
                    return;
                }
                self.selection = Some(Selection {
                    start: (col, row),
                    end: (col, row),
                });
                self.dragging = true;
                self.dirty = true;
                unsafe {
                    ffi::SetCapture(self.hwnd);
                }
            }
            0x0200 if self.dragging => {
                if let Some(selection) = self.selection.as_mut() {
                    selection.end = (col, row);
                    self.dirty = true;
                }
            }
            0x0202 if self.dragging => {
                self.dragging = false;
                unsafe {
                    ffi::ReleaseCapture();
                }
            }
            0x0207 => self.paste(),
            0x020a => {
                let delta = (wparam >> 16) as i16 as i32;
                let lines = (delta.unsigned_abs() as usize / 120).max(1) * 3;
                self.history = if delta > 0 {
                    self.history
                        .saturating_add(lines)
                        .min(self.core.terminal.history_len())
                } else {
                    self.history.saturating_sub(lines)
                };
                self.selection = None;
                self.dirty = true;
            }
            _ => {}
        }
    }
    fn open_link(&self, col: usize, row: usize) {
        let id = self.core.terminal.view_cell(col, row, self.history).link_id;
        let Some(uri) = self.core.terminal.hyperlink(id) else {
            return;
        };
        if uri.len() > 8192
            || uri.chars().any(char::is_control)
            || !(uri.starts_with("https://")
                || uri.starts_with("http://")
                || uri.starts_with("mailto:"))
        {
            return;
        }
        unsafe {
            ffi::ShellExecuteW(
                self.hwnd,
                wide("open").as_ptr(),
                wide(uri).as_ptr(),
                ptr::null(),
                ptr::null(),
                1,
            );
        }
    }
}

fn modifiers() -> Modifiers {
    let down = |vk| unsafe { ffi::GetKeyState(vk) } < 0;
    Modifiers(
        u8::from(down(0x10))
            | (u8::from(down(0x12)) << 1)
            | (u8::from(down(0x11)) << 2)
            | (u8::from(down(0x5b) || down(0x5c)) << 3)
            | (u8::from(unsafe { ffi::GetKeyState(0x14) } & 1 != 0) << 6)
            | (u8::from(unsafe { ffi::GetKeyState(0x90) } & 1 != 0) << 7),
    )
}

fn virtual_key(vk: u32, flags: isize) -> Key {
    match vk {
        0x1b => Key::Escape,
        0x0d if flags & (1 << 24) != 0 => Key::KeypadEnter,
        0x0d => Key::Enter,
        0x09 => Key::Tab,
        0x08 => Key::Backspace,
        0x25 => Key::Left,
        0x26 => Key::Up,
        0x27 => Key::Right,
        0x28 => Key::Down,
        0x24 => Key::Home,
        0x23 => Key::End,
        0x2d => Key::Insert,
        0x2e => Key::Delete,
        0x21 => Key::PageUp,
        0x22 => Key::PageDown,
        0x70..=0x87 => Key::Function((vk - 0x70 + 1) as u8),
        0x14 => Key::CapsLock,
        0x90 => Key::NumLock,
        0x91 => Key::ScrollLock,
        0x2c => Key::PrintScreen,
        0x13 => Key::Pause,
        0x5d => Key::Menu,
        0x10 => Key::Modifier(if (flags >> 16) & 0xff == 0x36 {
            ModifierKey::RightShift
        } else {
            ModifierKey::LeftShift
        }),
        0x11 => Key::Modifier(if flags & (1 << 24) != 0 {
            ModifierKey::RightControl
        } else {
            ModifierKey::LeftControl
        }),
        0x12 => Key::Modifier(if flags & (1 << 24) != 0 {
            ModifierKey::RightAlt
        } else {
            ModifierKey::LeftAlt
        }),
        0x5b => Key::Modifier(ModifierKey::LeftSuper),
        0x5c => Key::Modifier(ModifierKey::RightSuper),
        0x60..=0x69 => Key::KeypadDigit((vk - 0x60) as u8),
        0x6e => Key::KeypadDecimal,
        0x6f => Key::KeypadDivide,
        0x6a => Key::KeypadMultiply,
        0x6d => Key::KeypadSubtract,
        0x6b => Key::KeypadAdd,
        0x6c => Key::KeypadSeparator,
        _ => {
            let scalar = unsafe { ffi::MapVirtualKeyW(vk, 2) } & 0x7fffffff;
            char::from_u32(scalar)
                .filter(|c| !c.is_control())
                .map(|c| Key::Character(c.to_ascii_lowercase()))
                .unwrap_or(Key::Unidentified)
        }
    }
}

/// Win32 may synchronously dispatch nested messages. The guard ensures the
/// same App never acquires overlapping mutable references on those callbacks.
struct WindowState {
    app: UnsafeCell<App>,
    dispatching: Cell<bool>,
}
impl WindowState {
    fn with<R>(&self, operation: impl FnOnce(&mut App) -> R) -> Option<R> {
        if self.dispatching.replace(true) {
            return None;
        }
        let result = operation(unsafe { &mut *self.app.get() });
        self.dispatching.set(false);
        Some(result)
    }
}

unsafe extern "system" fn window_proc(
    hwnd: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if message == 0x0081 {
        let state = unsafe { *(lparam as *const *mut WindowState) };
        unsafe {
            ffi::SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
            (*(*state).app.get()).hwnd = hwnd;
        }
        return 1;
    }
    let pointer = unsafe { ffi::GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
    if pointer.is_null() {
        return unsafe { ffi::DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    let state = unsafe { &*pointer };
    let handled = state.with(|app| {
        match message {
            WM_PAINT => app.paint(),
            0x0014 => return Some(1),
            WM_SIZE if wparam != 1 => app.resize(),
            WM_CLOSE => {
                app.running = false;
                return Some(0);
            }
            WM_DESTROY => {
                app.running = false;
                unsafe {
                    ffi::PostQuitMessage(0);
                }
            }
            0x0007 | 0x0008 => {
                app.focused = message == 0x0007;
                app.encoded = input::encode_focus(app.focused, &app.core.terminal.modes().input());
                app.enqueue_encoded();
                app.reset_cursor();
                if app.focused {
                    app.ime_position();
                } else {
                    app.held.fill(None);
                    app.intercepted.fill(false);
                }
            }
            WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP => {
                if !app.key(message, wparam, lparam) {
                    return None;
                }
            }
            WM_CHAR | WM_SYSCHAR => app.character(wparam as u16),
            0x0109 => {
                if wparam == 0xffff {
                    return Some(1);
                }
                if let Some(ch) = char::from_u32(wparam as u32) {
                    let mut units = [0u16; 2];
                    for &unit in ch.encode_utf16(&mut units).iter() {
                        app.character(unit);
                    }
                }
            }
            0x0200..=0x020a => app.mouse(message, wparam, lparam),
            0x0215 => app.dragging = false,
            0x02e0 => {
                let dpi = wparam as u16 as u32;
                let candidate = Fonts::new(app.surface.dc, &app.appearance.font, dpi).ok();
                let rect = unsafe { &*(lparam as *const ffi::Rect) };
                unsafe {
                    ffi::SetWindowPos(
                        hwnd,
                        NULL,
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        0x0014,
                    );
                }
                app.resize_with_fonts(candidate);
            }
            0x0082 => {
                unsafe {
                    ffi::SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
                return None;
            }
            _ => return None,
        }
        Some(0)
    });
    handled
        .flatten()
        .unwrap_or_else(|| unsafe { ffi::DefWindowProcW(hwnd, message, wparam, lparam) })
}

pub fn run(cfg: Config) -> Result<i32, &'static str> {
    unsafe {
        ffi::SetProcessDpiAwarenessContext(-4isize as Handle);
    }
    let program = os::resolve(&cfg.command[0])?;
    let arguments: Vec<Vec<u16>> = cfg
        .command
        .iter()
        .map(|s| s.to_string_lossy().encode_utf16().collect())
        .collect();
    let arguments: Vec<&[u16]> = arguments.iter().map(Vec::as_slice).collect();
    let environment = os::child_environment();
    let pty = zpty::Pty::spawn_utf16(
        &program,
        &arguments,
        Some(&environment),
        zpty::Size::new(cfg.cols as u16, cfg.rows as u16),
    )
    .map_err(|_| "cannot start the Windows pseudoconsole (Windows 10 1809 or newer required)")?;
    let surface = Surface::new()?;
    let fonts = Fonts::new(surface.dc, &cfg.appearance.font, unsafe {
        ffi::GetDpiForSystem()
    })?;
    let width = cfg.cols as i32 * fonts.width + cfg.appearance.padding * 2;
    let height = cfg.rows as i32 * fonts.height + cfg.appearance.padding * 2;
    let mut terminal = Terminal::with_theme(cfg.cols, cfg.rows, cfg.history, cfg.appearance.theme);
    terminal.set_cell_size(fonts.width as u32, fonts.height as u32);
    let state = Box::new(WindowState {
        app: UnsafeCell::new(App {
            hwnd: NULL,
            core: SessionCore::new(pty, terminal, String::from("local"), MAX_INPUT),
            appearance: cfg.appearance,
            surface,
            fonts,
            encoded: Vec::with_capacity(128),
            running: true,
            dirty: true,
            history: 0,
            selection: None,
            dragging: false,
            held: [None; 256],
            intercepted: [false; 256],
            pending_key: None,
            suppress_char: false,
            surrogate: 0,
            title: String::new(),
            graphics_revision: 0,
            sync_since: None,
            cursor_on: true,
            blink_deadline: os::milliseconds() + 500,
            last_cursor: (0, 0),
            focused: false,
        }),
        dispatching: Cell::new(false),
    });
    let class_name = wide("ztty.NativeWindow");
    let instance = unsafe { ffi::GetModuleHandleW(ptr::null()) };
    let class = ffi::WindowClass {
        size: core::mem::size_of::<ffi::WindowClass>() as u32,
        style: 3,
        procedure: Some(window_proc),
        class_extra: 0,
        window_extra: 0,
        instance,
        icon: NULL,
        cursor: unsafe { ffi::LoadCursorW(NULL, 32513usize as *const u16) },
        background: NULL,
        menu: ptr::null(),
        name: class_name.as_ptr(),
        small_icon: NULL,
    };
    if unsafe { ffi::RegisterClassExW(&class) } == 0 {
        return Err("cannot register the native window class");
    }
    let mut rect = ffi::Rect {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    unsafe {
        ffi::AdjustWindowRectEx(&mut rect, WINDOW_STYLE, 0, 0);
    }
    let hwnd = unsafe {
        ffi::CreateWindowExW(
            0,
            class_name.as_ptr(),
            wide("ztty").as_ptr(),
            WINDOW_STYLE,
            i32::MIN,
            i32::MIN,
            rect.right - rect.left,
            rect.bottom - rect.top,
            NULL,
            NULL,
            instance,
            (&*state as *const WindowState).cast_mut().cast(),
        )
    };
    if hwnd.is_null() {
        unsafe {
            ffi::UnregisterClassW(class_name.as_ptr(), instance);
        }
        return Err("cannot create a native Windows window");
    }
    let dark = 1i32;
    unsafe {
        ffi::DwmSetWindowAttribute(hwnd, 20, (&dark as *const i32).cast(), 4);
        ffi::ShowWindow(hwnd, 5);
    }
    state.with(App::resize);
    loop {
        let mut message: ffi::Message = unsafe { core::mem::zeroed() };
        let mut quit = false;
        for _ in 0..512 {
            if unsafe { ffi::PeekMessageW(&mut message, NULL, 0, 0, 1) } == 0 {
                break;
            }
            if message.message == 0x0012 {
                quit = true;
                break;
            }
            unsafe {
                ffi::TranslateMessage(&message);
                ffi::DispatchMessageW(&message);
            }
        }
        if quit {
            break;
        }
        let running = state
            .with(|app| {
                app.tick();
                app.running
            })
            .unwrap_or(false);
        if !running {
            break;
        }
        unsafe {
            ffi::UpdateWindow(hwnd);
        }
        let (events, timeout) = state
            .with(|app| {
                let now = os::milliseconds();
                let mut timeout = if app.focused
                    && app
                        .core
                        .terminal
                        .modes()
                        .cursor_blink
                        .unwrap_or(app.appearance.cursor_blink)
                {
                    app.blink_deadline.saturating_sub(now).min(500) as u32
                } else {
                    1000
                };
                if app.core.input.pending() {
                    timeout = timeout.min(8);
                }
                if app.sync_since.is_some() {
                    timeout = timeout.min(8);
                }
                (
                    [app.core.pty.output_event(), app.core.pty.process_event()],
                    timeout,
                )
            })
            .unwrap_or(([NULL, NULL], 8));
        unsafe {
            ffi::MsgWaitForMultipleObjectsEx(2, events.as_ptr(), timeout, 0x04ff, 4);
        }
    }
    let status = state
        .with(|app| {
            let _ = app.core.pty.wait_timeout(20);
            app.core.pty.status().and_then(|s| s.code()).unwrap_or(0)
        })
        .unwrap_or(0);
    unsafe {
        ffi::DestroyWindow(hwnd);
        ffi::UnregisterClassW(class_name.as_ptr(), instance);
    }
    Ok(status)
}

#[allow(non_snake_case)]
mod ffi {
    use super::*;
    #[repr(C)]
    #[derive(Default)]
    pub struct Point {
        pub x: i32,
        pub y: i32,
    }
    #[repr(C)]
    #[derive(Default)]
    pub struct Rect {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }
    #[repr(C)]
    pub struct WindowClass {
        pub size: u32,
        pub style: u32,
        pub procedure: Option<unsafe extern "system" fn(Handle, u32, usize, isize) -> isize>,
        pub class_extra: i32,
        pub window_extra: i32,
        pub instance: Handle,
        pub icon: Handle,
        pub cursor: Handle,
        pub background: Handle,
        pub menu: *const u16,
        pub name: *const u16,
        pub small_icon: Handle,
    }
    #[repr(C)]
    pub struct Message {
        pub hwnd: Handle,
        pub message: u32,
        pub wparam: usize,
        pub lparam: isize,
        pub time: u32,
        pub point: Point,
        pub private: u32,
    }
    #[repr(C)]
    pub struct PaintStruct {
        pub dc: Handle,
        pub erase: i32,
        pub paint: Rect,
        pub restore: i32,
        pub update: i32,
        pub reserved: [u8; 32],
    }
    #[repr(C)]
    pub struct BitmapInfoHeader {
        pub size: u32,
        pub width: i32,
        pub height: i32,
        pub planes: u16,
        pub bits: u16,
        pub compression: u32,
        pub image_size: u32,
        pub x_ppm: i32,
        pub y_ppm: i32,
        pub colors: u32,
        pub important: u32,
    }
    #[repr(C)]
    pub struct BitmapInfo {
        pub header: BitmapInfoHeader,
        pub palette: [u32; 1],
    }
    #[repr(C)]
    pub struct TextMetric {
        pub height: i32,
        pub ascent: i32,
        pub descent: i32,
        pub internal_leading: i32,
        pub external_leading: i32,
        pub average_width: i32,
        pub max_width: i32,
        pub weight: i32,
        pub overhang: i32,
        pub aspect_x: i32,
        pub aspect_y: i32,
        pub first: u16,
        pub last: u16,
        pub default: u16,
        pub break_char: u16,
        pub italic: u8,
        pub underlined: u8,
        pub struck_out: u8,
        pub pitch: u8,
        pub charset: u8,
    }
    #[repr(C)]
    pub struct CompositionForm {
        pub style: u32,
        pub position: Point,
        pub area: Rect,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn SetProcessDpiAwarenessContext(context: Handle) -> i32;
        pub fn GetDpiForSystem() -> u32;
        pub fn RegisterClassExW(class: *const WindowClass) -> u16;
        pub fn UnregisterClassW(name: *const u16, instance: Handle) -> i32;
        pub fn CreateWindowExW(
            ex: u32,
            class: *const u16,
            name: *const u16,
            style: u32,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            parent: Handle,
            menu: Handle,
            instance: Handle,
            parameter: *mut c_void,
        ) -> Handle;
        pub fn DefWindowProcW(hwnd: Handle, message: u32, wparam: usize, lparam: isize) -> isize;
        pub fn SetWindowLongPtrW(hwnd: Handle, index: i32, value: isize) -> isize;
        pub fn GetWindowLongPtrW(hwnd: Handle, index: i32) -> isize;
        pub fn AdjustWindowRectEx(rect: *mut Rect, style: u32, menu: i32, ex: u32) -> i32;
        pub fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
        pub fn ShowWindow(hwnd: Handle, show: i32) -> i32;
        pub fn DestroyWindow(hwnd: Handle) -> i32;
        pub fn UpdateWindow(hwnd: Handle) -> i32;
        pub fn GetClientRect(hwnd: Handle, rect: *mut Rect) -> i32;
        pub fn InvalidateRect(hwnd: Handle, rect: *const Rect, erase: i32) -> i32;
        pub fn BeginPaint(hwnd: Handle, paint: *mut PaintStruct) -> Handle;
        pub fn EndPaint(hwnd: Handle, paint: *const PaintStruct) -> i32;
        pub fn SetWindowTextW(hwnd: Handle, text: *const u16) -> i32;
        pub fn SetWindowPos(
            hwnd: Handle,
            after: Handle,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> i32;
        pub fn PeekMessageW(
            message: *mut Message,
            hwnd: Handle,
            min: u32,
            max: u32,
            remove: u32,
        ) -> i32;
        pub fn TranslateMessage(message: *const Message) -> i32;
        pub fn DispatchMessageW(message: *const Message) -> isize;
        pub fn PostQuitMessage(code: i32);
        pub fn MsgWaitForMultipleObjectsEx(
            count: u32,
            handles: *const Handle,
            timeout: u32,
            wake: u32,
            flags: u32,
        ) -> u32;
        pub fn GetKeyState(vk: i32) -> i16;
        pub fn MapVirtualKeyW(code: u32, map: u32) -> u32;
        pub fn SetCapture(hwnd: Handle) -> Handle;
        pub fn ReleaseCapture() -> i32;
        pub fn ScreenToClient(hwnd: Handle, point: *mut Point) -> i32;
        pub fn OpenClipboard(hwnd: Handle) -> i32;
        pub fn EmptyClipboard() -> i32;
        pub fn SetClipboardData(format: u32, memory: Handle) -> Handle;
        pub fn GetClipboardData(format: u32) -> Handle;
        pub fn CloseClipboard() -> i32;
    }
    #[link(name = "gdi32")]
    unsafe extern "system" {
        pub fn CreateCompatibleDC(dc: Handle) -> Handle;
        pub fn DeleteDC(dc: Handle) -> i32;
        pub fn SelectObject(dc: Handle, object: Handle) -> Handle;
        pub fn DeleteObject(object: Handle) -> i32;
        pub fn CreateDIBSection(
            dc: Handle,
            info: *const BitmapInfo,
            usage: u32,
            bits: *mut *mut c_void,
            section: Handle,
            offset: u32,
        ) -> Handle;
        pub fn CreateFontW(
            height: i32,
            width: i32,
            escapement: i32,
            orientation: i32,
            weight: i32,
            italic: u32,
            underline: u32,
            strike: u32,
            charset: u32,
            out: u32,
            clip: u32,
            quality: u32,
            pitch: u32,
            face: *const u16,
        ) -> Handle;
        pub fn GetTextMetricsW(dc: Handle, metrics: *mut TextMetric) -> i32;
        pub fn SetBkMode(dc: Handle, mode: i32) -> i32;
        pub fn SetTextColor(dc: Handle, color: u32) -> u32;
        pub fn ExtTextOutW(
            dc: Handle,
            x: i32,
            y: i32,
            options: u32,
            rect: *const Rect,
            text: *const u16,
            count: u32,
            advances: *const i32,
        ) -> i32;
        pub fn BitBlt(
            target: Handle,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            source: Handle,
            sx: i32,
            sy: i32,
            operation: u32,
        ) -> i32;
        pub fn GdiFlush() -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GetModuleHandleW(name: *const u16) -> Handle;
        pub fn GlobalAlloc(flags: u32, size: usize) -> Handle;
        pub fn GlobalLock(memory: Handle) -> Handle;
        pub fn GlobalUnlock(memory: Handle) -> i32;
        pub fn GlobalSize(memory: Handle) -> usize;
        pub fn GlobalFree(memory: Handle) -> Handle;
    }
    #[link(name = "shell32")]
    unsafe extern "system" {
        pub fn ShellExecuteW(
            hwnd: Handle,
            verb: *const u16,
            file: *const u16,
            arguments: *const u16,
            directory: *const u16,
            show: i32,
        ) -> Handle;
    }
    #[link(name = "dwmapi")]
    unsafe extern "system" {
        pub fn DwmSetWindowAttribute(
            hwnd: Handle,
            attribute: u32,
            value: *const c_void,
            size: u32,
        ) -> i32;
    }
    #[link(name = "imm32")]
    unsafe extern "system" {
        pub fn ImmGetContext(hwnd: Handle) -> Handle;
        pub fn ImmReleaseContext(hwnd: Handle, context: Handle) -> i32;
        pub fn ImmSetCompositionWindow(context: Handle, form: *const CompositionForm) -> i32;
    }
}
