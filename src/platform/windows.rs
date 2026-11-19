//! Native Win32 frontend. GDI draws text into a retained DIB; the image path
//! composites decoded RGBA pixels itself. ConPTY pipe work lives in zpty.

use crate::{Config, appearance::Appearance, os};
use alloc::{boxed::Box, ffi::CString, format, string::String, sync::Arc, vec, vec::Vec};
use core::{
    cell::{Cell, UnsafeCell},
    ffi::c_void,
    ptr,
};
use zterm::{
    CursorShape, Terminal,
    input::{self, Key, KeyEvent, ModifierKey, Modifiers, MouseEvent},
};

use ztty::{
    actions,
    layout::{Axis, Layout, Rect},
    native_resize as session_resize,
    session_core::{ReadStep, SessionCore},
};

#[path = "windows_dispatch.rs"]
mod session_dispatch;
#[path = "windows_frame.rs"]
mod session_frame;
#[path = "windows_wait.rs"]
mod session_wait;

type Handle = *mut c_void;
const NULL: Handle = ptr::null_mut();
const WINDOW_STYLE: u32 = 0x00cf0000;
const MAX_INPUT: usize = 8 * 1024 * 1024;
const MAX_SURFACE_PIXELS: usize = 16 * 1024 * 1024;
const GWLP_USERDATA: i32 = -21;
const WM_PAINT: u32 = 0x000f;
const WM_CLOSE: u32 = 0x0010;
const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_NCCALCSIZE: u32 = 0x0083;
const WM_WINDOWPOSCHANGING: u32 = 0x0046;
const WVR_VALIDRECTS: isize = 0x0400;
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOREDRAW: u32 = 0x0008;
const SWP_NOCOPYBITS: u32 = 0x0100;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_CHAR: u32 = 0x0102;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_SYSKEYUP: u32 = 0x0105;
const WM_SYSCHAR: u32 = 0x0106;
const WM_NATIVE_SNAPSHOT: u32 = 0x85a1;
const WM_NATIVE_PAINT_SAMPLE: u32 = 0x85a2;
const NATIVE_SNAPSHOT_MAGIC: usize = 0x5a545459;

#[derive(Default)]
struct PaintRecord {
    sequence: u64,
    surface: [i32; 2],
    region: [i32; 4],
    blit: i32,
    start_event_sequence: u64,
    end_event_sequence: u64,
    default_message: u32,
    default_depth: u32,
    target: Option<PaintTarget>,
    target_after_end: Option<PaintAfterEnd>,
}

struct PaintTarget {
    sample: [i32; 2],
    cpu_before_blit: Option<u32>,
    flush_after_blit: i32,
    target_after_flush: Option<u32>,
    source_after_target_read: Option<u32>,
    clip_type: i32,
    clip: [i32; 4],
    origin_ok: bool,
    origin: [i32; 2],
}

struct PaintAfterEnd {
    sample: [i32; 2],
    sample_end_event_sequence: u64,
    end_paint_return: i32,
    client_dc_ok: bool,
    target_rgb: Option<u32>,
    release_dc_return: Option<i32>,
}

struct SurfaceResizeRecord {
    count: u64,
    from: [i32; 2],
    to: [i32; 2],
}

const POSITION_MESSAGES: [u32; 6] = [
    WM_WINDOWPOSCHANGING,
    0x0047,
    0x0088,
    0x0085,
    0x0014,
    WM_SIZE,
];

#[derive(Clone, Copy, Default)]
struct MessagePhase {
    received: u64,
    default_enter: u64,
    default_exit: u64,
}

#[derive(Clone, Copy, Default)]
struct PositionEvent {
    sequence: u64,
    flags_in: u32,
    flags_submitted: u32,
    position: [i32; 4],
    current_outer: Option<[i32; 4]>,
}
impl PositionEvent {
    fn json(self) -> String {
        let outer = self.current_outer.map_or_else(
            || String::from("null"),
            |rect| format!("[{},{},{},{}]", rect[0], rect[1], rect[2], rect[3]),
        );
        format!(
            concat!(
                "{{\"sequence\":{},\"flags_in\":{},\"flags_submitted\":{},",
                "\"position\":[{},{},{},{}],\"current_outer\":{}}}"
            ),
            self.sequence,
            self.flags_in,
            self.flags_submitted,
            self.position[0],
            self.position[1],
            self.position[2],
            self.position[3],
            outer
        )
    }
}

#[derive(Clone, Copy)]
struct NcCalcStage {
    rects: [[i32; 4]; 3],
    position: Option<ffi::WindowPos>,
}
impl NcCalcStage {
    fn capture(params: &ffi::NcCalcSizeParams) -> Self {
        Self {
            rects: params
                .rects
                .each_ref()
                .map(|rect| [rect.left, rect.top, rect.right, rect.bottom]),
            position: unsafe { params.position.as_ref() }.copied(),
        }
    }
    fn json(self) -> String {
        let rects = self
            .rects
            .map(|rect| format!("[{},{},{},{}]", rect[0], rect[1], rect[2], rect[3]));
        let position = self.position.map_or_else(
            || String::from("null"),
            |position| {
                format!(
                    concat!(
                        "{{\"hwnd\":{},\"insert_after\":{},\"x\":{},\"y\":{},",
                        "\"width\":{},\"height\":{},\"flags\":{}}}"
                    ),
                    position.hwnd as usize,
                    position.insert_after as usize,
                    position.x,
                    position.y,
                    position.width,
                    position.height,
                    position.flags
                )
            },
        );
        format!(
            "{{\"rects\":[{},{},{}],\"lppos\":{}}}",
            rects[0], rects[1], rects[2], position
        )
    }
}

#[derive(Clone, Copy)]
struct NcCalcEvent {
    sequence: u64,
    wparam: usize,
    input: NcCalcStage,
    default_return: isize,
    after_default: NcCalcStage,
    submitted_return: isize,
    submitted: NcCalcStage,
}
impl NcCalcEvent {
    fn json(self) -> String {
        format!(
            concat!(
                "{{\"sequence\":{},\"wparam\":{},\"input\":{},\"default_return\":{},",
                "\"after_default\":{},\"submitted_return\":{},\"submitted\":{}}}"
            ),
            self.sequence,
            self.wparam,
            self.input.json(),
            self.default_return,
            self.after_default.json(),
            self.submitted_return,
            self.submitted.json()
        )
    }
}

#[derive(Clone, Copy, Default)]
struct NativeEventRecord {
    messages: [MessagePhase; 6],
    changing: PositionEvent,
    changed: PositionEvent,
    size: [u32; 3],
    default_message: u32,
    default_depth: u32,
    nccalc_valid_rects: Option<NcCalcEvent>,
}

/// Memory-only ordering evidence. Every Cell update finishes before a native
/// call can reenter; formatting happens only for the failure snapshot.
struct NativeEvents {
    enabled: bool,
    paint_diagnostics: bool,
    paint_sample: Cell<Option<[i32; 2]>>,
    sequence: Cell<u64>,
    record: Cell<NativeEventRecord>,
}
impl NativeEvents {
    fn new(enabled: bool, paint_diagnostics: bool) -> Self {
        Self {
            enabled,
            paint_diagnostics,
            paint_sample: Cell::new(None),
            sequence: Cell::new(0),
            record: Cell::new(NativeEventRecord::default()),
        }
    }
    fn next(&self) -> u64 {
        if !self.enabled {
            return 0;
        }
        let sequence = self.sequence.get().saturating_add(1);
        self.sequence.set(sequence);
        sequence
    }
    fn update(&self, update: impl FnOnce(&mut NativeEventRecord)) {
        if self.enabled {
            let mut record = self.record.get();
            update(&mut record);
            self.record.set(record);
        }
    }
    fn received(&self, message: u32) -> u64 {
        if !self.enabled {
            return 0;
        }
        let sequence = self.next();
        if let Some(index) = POSITION_MESSAGES.iter().position(|value| *value == message) {
            self.update(|record| record.messages[index].received = sequence);
        }
        sequence
    }
    fn enter_default(&self, message: u32) -> u32 {
        if !self.enabled {
            return 0;
        }
        let previous = self.record.get().default_message;
        let sequence = self.next();
        self.update(|record| {
            record.default_depth = record.default_depth.saturating_add(1);
            record.default_message = message;
            if let Some(index) = POSITION_MESSAGES.iter().position(|value| *value == message) {
                record.messages[index].default_enter = sequence;
            }
        });
        previous
    }
    fn exit_default(&self, message: u32, previous: u32) {
        if !self.enabled {
            return;
        }
        let sequence = self.next();
        self.update(|record| {
            record.default_depth = record.default_depth.saturating_sub(1);
            record.default_message = previous;
            if let Some(index) = POSITION_MESSAGES.iter().position(|value| *value == message) {
                record.messages[index].default_exit = sequence;
            }
        });
    }
    fn json(&self) -> String {
        let record = self.record.get();
        let mut messages = String::new();
        for (index, message) in POSITION_MESSAGES.iter().enumerate() {
            if index != 0 {
                messages.push(',');
            }
            let phase = record.messages[index];
            messages.push_str(&format!(
                "{{\"message\":{},\"received\":{},\"default_enter\":{},\"default_exit\":{}}}",
                message, phase.received, phase.default_enter, phase.default_exit
            ));
        }
        let changed = record.changed;
        let changed = format!(
            "{{\"sequence\":{},\"flags_reported\":{},\"position\":[{},{},{},{}]}}",
            changed.sequence,
            changed.flags_in,
            changed.position[0],
            changed.position[1],
            changed.position[2],
            changed.position[3]
        );
        format!(
            concat!(
                "{{\"enabled\":{},\"sequence\":{},\"default_message\":{},\"default_depth\":{},",
                "\"messages\":[{}],\"changing\":{},\"changed\":{},\"nccalc_valid_rects\":{},",
                "\"last_size\":{{\"kind\":{},\"size\":[{},{}]}}}}"
            ),
            self.enabled,
            self.sequence.get(),
            record.default_message,
            record.default_depth,
            messages,
            record.changing.json(),
            changed,
            record
                .nccalc_valid_rects
                .map_or_else(|| String::from("null"), NcCalcEvent::json),
            record.size[0],
            record.size[1],
            record.size[2]
        )
    }
}

struct NativeTrace {
    remaining: Cell<u16>,
}
impl NativeTrace {
    fn new(enabled: bool) -> Self {
        Self {
            remaining: Cell::new(if enabled { 64 } else { 0 }),
        }
    }
    fn enabled(&self) -> bool {
        self.remaining.get() != 0
    }
    fn record(&self, message: core::fmt::Arguments<'_>) {
        if self.enabled() {
            self.remaining.set(self.remaining.get() - 1);
            os::print(2, format!("{message}\n").as_bytes());
        }
    }
}

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
    clip: Rect,
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
            clip: Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            width: 0,
            height: 0,
        })
    }
    fn resize(&mut self, width: i32, height: i32) -> bool {
        if width == self.width && height == self.height {
            return true;
        }
        if width <= 0 || height <= 0 || (width as u64) * (height as u64) > MAX_SURFACE_PIXELS as u64
        {
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
        self.clip = Rect {
            x: 0,
            y: 0,
            width,
            height,
        };
        self.fill(0, 0, width, height, 0);
        true
    }
    fn snapshot(
        &self,
        viewport: Rect,
        budget: usize,
    ) -> Result<session_frame::Snapshot, &'static str> {
        if self.pixels.is_null() || self.width <= 0 || self.height <= 0 {
            return Err("no committed workspace surface is available");
        }
        // CreateDIBSection requires the GDI queue to finish before CPU access.
        unsafe {
            ffi::GdiFlush();
        }
        let pixels = unsafe {
            core::slice::from_raw_parts(self.pixels, self.width as usize * self.height as usize)
        };
        session_frame::Snapshot::capture(pixels, self.width as usize, viewport, budget)
    }
    fn restore_snapshot(&mut self, snapshot: &session_frame::Snapshot, viewport: Rect) {
        if self.pixels.is_null() {
            return;
        }
        unsafe {
            ffi::GdiFlush();
        }
        let pixels = unsafe {
            core::slice::from_raw_parts_mut(self.pixels, self.width as usize * self.height as usize)
        };
        snapshot.blit(pixels, self.width as usize, viewport);
    }
    fn fill(&mut self, x: i32, y: i32, width: i32, height: i32, rgb: u32) {
        if self.pixels.is_null() {
            return;
        }
        let left = x.max(self.clip.x).clamp(0, self.width);
        let right = x
            .saturating_add(width)
            .min(self.clip.x.saturating_add(self.clip.width))
            .clamp(0, self.width);
        let top = y.max(self.clip.y).clamp(0, self.height);
        let bottom = y
            .saturating_add(height)
            .min(self.clip.y.saturating_add(self.clip.height))
            .clamp(0, self.height);
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
        viewport: Rect,
        padding: i32,
        grid: (usize, usize),
        cell_size: (i32, i32),
    ) {
        let (cols, rows) = grid;
        let (cell_width, cell_height) = cell_size;
        if self.pixels.is_null()
            || draw.dest.width == 0
            || draw.dest.height == 0
            || draw.source.width == 0
            || draw.source.height == 0
        {
            return;
        }
        let px = i64::from(viewport.x) + i64::from(padding);
        let py = i64::from(viewport.y) + i64::from(padding);
        let dx = i64::from(draw.dest.x) + px;
        let dy = i64::from(draw.dest.y) + py;
        let left = dx.max(px).max(self.clip.x as i64).max(0);
        let top = dy.max(py).max(self.clip.y as i64).max(0);
        let right = (dx + i64::from(draw.dest.width))
            .min(px + cols as i64 * i64::from(cell_width))
            .min(i64::from(self.clip.x) + i64::from(self.clip.width))
            .min(self.width as i64);
        let bottom = (dy + i64::from(draw.dest.height))
            .min(py + rows as i64 * i64::from(cell_height))
            .min(i64::from(self.clip.y) + i64::from(self.clip.height))
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
    description: String,
    size: u32,
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
            description: String::from(description),
            size,
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
        ztty::selection::text(terminal, self.start, self.end, history)
    }
}

#[derive(Clone, Copy)]
struct PendingKey {
    key: Key,
    modifiers: Modifiers,
    event: KeyEvent,
}

struct Pane {
    // Drop waits before the handles owned by core. Callbacks never touch App.
    output_wait: session_wait::Wait,
    _process_wait: session_wait::Wait,
    id: u64,
    visible: bool,
    pty_size: zpty::Size,
    title: String,
    rect: Rect,
    core: SessionCore,
    dirty: bool,
    history: usize,
    selection: Option<Selection>,
    dragging: bool,
    graphics_revision: u64,
    sync_since: Option<u64>,
    held_frame: Option<session_frame::Snapshot>,
    cursor_on: bool,
    blink_deadline: u64,
    last_cursor: (usize, usize),
}

impl Pane {
    fn new(
        id: u64,
        pty: zpty::Pty,
        terminal: Terminal,
        size: zpty::Size,
        rect: Rect,
        wake: &Arc<session_wait::Wake>,
    ) -> Result<Self, &'static str> {
        let output_wait = session_wait::Wait::new(pty.output_event(), wake)?;
        let process_wait = session_wait::Wait::new(pty.process_event(), wake)?;
        Ok(Self {
            output_wait,
            _process_wait: process_wait,
            id,
            visible: true,
            pty_size: size,
            title: String::new(),
            rect,
            core: SessionCore::new(pty, terminal, String::from("local"), MAX_INPUT),
            dirty: true,
            history: 0,
            selection: None,
            dragging: false,
            graphics_revision: 0,
            sync_since: None,
            held_frame: None,
            cursor_on: true,
            blink_deadline: os::milliseconds() + 500,
            last_cursor: (0, 0),
        })
    }

    fn grid_ready(&self) -> bool {
        self.core.terminal.cols() == usize::from(self.pty_size.cols)
            && self.core.terminal.rows() == usize::from(self.pty_size.rows)
    }
}

struct Launch {
    shell: CString,
    environment: Vec<u16>,
    history: usize,
}

struct App {
    hwnd: Handle,
    panes: Vec<Pane>,
    active: usize,
    layout: Layout,
    next_id: u64,
    launch: Launch,
    wake: Arc<session_wait::Wake>,
    pump_cursor: usize,
    pump_again: bool,
    pending_fonts: Option<Fonts>,
    resize_pending: bool,
    resize_retry_at: u64,
    full_redraw: bool,
    caption_dirty: bool,
    notice: Option<(String, u64)>,
    last_status: i32,
    appearance: Appearance,
    surface: Surface,
    fonts: Fonts,
    encoded: Vec<u8>,
    running: bool,
    held: [Option<Key>; 256],
    intercepted: [bool; 256],
    pending_key: Option<PendingKey>,
    suppress_char: bool,
    surrogate: u16,
    mouse_capture: Option<(u64, u8)>,
    cancelled_mouse_buttons: u8,
    title: String,
    focused: bool,
    native_trace: NativeTrace,
    native_snapshot_path: Option<Vec<u16>>,
    surface_resize: Option<SurfaceResizeRecord>,
    last_paint: PaintRecord,
}

impl App {
    /// An explicit diagnostic read of our DIB through the kernel, without asking
    /// GDI to flush its batch. The value may precede queued GDI work.
    fn diagnostic_cpu_pixel(&self, x: i32, y: i32) -> Option<u32> {
        if self.surface.pixels.is_null()
            || x < 0
            || y < 0
            || x >= self.surface.width
            || y >= self.surface.height
        {
            return None;
        }
        let mut pixel = 0u32;
        let mut read = 0;
        let ok = unsafe {
            ffi::ReadProcessMemory(
                -1isize as Handle,
                self.surface
                    .pixels
                    .add((y * self.surface.width + x) as usize)
                    .cast(),
                (&mut pixel as *mut u32).cast(),
                core::mem::size_of::<u32>(),
                &mut read,
            )
        };
        (ok != 0 && read == core::mem::size_of::<u32>()).then_some(pixel)
    }

    fn failure_snapshot(
        &mut self,
        x: i32,
        y: i32,
        dispatch_resize_pending: bool,
        dispatch_paint_pending: bool,
        deferred_paints: u64,
        events: &NativeEvents,
    ) -> isize {
        // Only an explicitly configured test process may write this one-shot
        // record. No path or pointer is accepted from a window message.
        let Some(path) = self.native_snapshot_path.take() else {
            return 0;
        };
        let rgb = |pixel: Option<u32>| {
            pixel.map_or_else(
                || String::from("null"),
                |value| {
                    format!(
                        "[{},{},{}]",
                        value >> 16 & 255,
                        value >> 8 & 255,
                        value & 255
                    )
                },
            )
        };
        let cpu = self.diagnostic_cpu_pixel(x, y);
        let target = self.last_paint.target.as_ref().map_or_else(|| String::from("null"), |target| {
            format!(concat!("{{\"sample\":[{},{}],\"cpu_before_blit_rgb\":{},\"gdi_flush_after_blit_return\":{},",
                "\"target_after_flush_rgb\":{},\"source_after_target_read_rgb\":{},",
                "\"clip_type\":{},\"clip\":[{},{},{},{}],\"origin_ok\":{},\"origin\":[{},{}]}}"),
                target.sample[0], target.sample[1], rgb(target.cpu_before_blit), target.flush_after_blit,
                rgb(target.target_after_flush), rgb(target.source_after_target_read),
                target.clip_type, target.clip[0], target.clip[1], target.clip[2], target.clip[3],
                target.origin_ok, target.origin[0], target.origin[1])
        });
        let target_after_end = self.last_paint.target_after_end.as_ref().map_or_else(
            || String::from("null"),
            |target| {
                format!(
                    concat!(
                        "{{\"sample\":[{},{}],\"sample_end_event_sequence\":{},\"end_paint_return\":{},\"client_dc_ok\":{},",
                        "\"target_rgb\":{},\"release_dc_return\":{}}}"
                    ),
                    target.sample[0],
                    target.sample[1],
                    target.sample_end_event_sequence,
                    target.end_paint_return,
                    target.client_dc_ok,
                    rgb(target.target_rgb),
                    target
                        .release_dc_return
                        .map_or_else(|| String::from("null"), |value| format!("{value}"))
                )
            },
        );
        let pane = self.panes.get(self.active).map_or_else(|| String::from("null"), |pane| {
            format!(concat!("{{\"rect\":[{},{},{},{}],\"grid\":[{},{}],\"pty\":[{},{}],\"images\":{},\"placements\":{},",
                            "\"dirty\":{},\"terminal_dirty\":{},\"grid_ready\":{},\"synchronized_output\":{}}}"),
                pane.rect.x, pane.rect.y, pane.rect.width, pane.rect.height,
                pane.core.terminal.cols(), pane.core.terminal.rows(), pane.pty_size.cols, pane.pty_size.rows,
                pane.core.terminal.graphics().image_count(), pane.core.terminal.graphics().placement_count(),
                pane.dirty, pane.core.terminal.dirty(), pane.grid_ready(), pane.core.terminal.modes().synchronized_output)
        });
        let surface_resize = self.surface_resize.as_ref().map_or_else(
            || String::from("null"),
            |resize| {
                format!(
                    "{{\"count\":{},\"last_from\":[{},{}],\"last_to\":[{},{}]}}",
                    resize.count, resize.from[0], resize.from[1], resize.to[0], resize.to[1]
                )
            },
        );
        let before = format!(
            concat!(
                "{{\"surface\":{{\"size\":[{},{}],\"clip\":[{},{},{},{}],\"resize_changes\":{}}},\"cpu_rgb\":{},\"dispatch_resize_pending\":{},",
                "\"dispatch_paint_pending\":{},\"deferred_paints\":{},",
                "\"app_resize_pending\":{},\"full_redraw\":{},\"pane\":{},\"last_paint\":{{\"sequence\":{},",
                "\"surface\":[{},{}],\"region\":[{},{},{},{}],\"blit\":{},",
                "\"start_event_sequence\":{},\"end_event_sequence\":{},\"default_message\":{},\"default_depth\":{},\"target_after_blit\":{},",
                "\"target_after_end_paint\":{}}},",
                "\"events\":{}}}"
            ),
            self.surface.width,
            self.surface.height,
            self.surface.clip.x,
            self.surface.clip.y,
            self.surface.clip.width,
            self.surface.clip.height,
            surface_resize,
            rgb(cpu),
            dispatch_resize_pending,
            dispatch_paint_pending,
            deferred_paints,
            self.resize_pending,
            self.full_redraw,
            pane,
            self.last_paint.sequence,
            self.last_paint.surface[0],
            self.last_paint.surface[1],
            self.last_paint.region[0],
            self.last_paint.region[1],
            self.last_paint.region[2],
            self.last_paint.region[3],
            self.last_paint.blit,
            self.last_paint.start_event_sequence,
            self.last_paint.end_event_sequence,
            self.last_paint.default_message,
            self.last_paint.default_depth,
            target,
            target_after_end,
            events.json()
        );
        // All App/CPU fields above are captured before these native queries.
        let mut client = ffi::Rect::default();
        let mut update = ffi::Rect::default();
        let client_ok = unsafe { ffi::GetClientRect(self.hwnd, &mut client) } != 0;
        let update_pending = unsafe { ffi::GetUpdateRect(self.hwnd, &mut update, 0) } != 0;
        let flush = unsafe { ffi::GdiFlush() };
        let after_cpu = self.diagnostic_cpu_pixel(x, y);
        let mut bitmap: ffi::Bitmap = unsafe { core::mem::zeroed() };
        let selected = unsafe { ffi::GetCurrentObject(self.surface.dc, 7) };
        let bitmap_read = unsafe {
            ffi::GetObjectW(
                selected,
                core::mem::size_of::<ffi::Bitmap>() as i32,
                (&mut bitmap as *mut ffi::Bitmap).cast(),
            )
        };
        let sample = |dc: Handle| {
            if dc.is_null() {
                return None;
            }
            let value = unsafe { ffi::GetPixel(dc, x, y) };
            (value != u32::MAX).then(|| colorref(value))
        };
        let source = sample(self.surface.dc);
        let mut source_clip = ffi::Rect::default();
        let source_clip_type = unsafe { ffi::GetClipBox(self.surface.dc, &mut source_clip) };
        let target = unsafe { ffi::GetDC(self.hwnd) };
        let target_rgb = sample(target);
        let mut target_clip = ffi::Rect::default();
        let target_clip_type = if target.is_null() {
            0
        } else {
            unsafe { ffi::GetClipBox(target, &mut target_clip) }
        };
        if !target.is_null() {
            unsafe {
                ffi::ReleaseDC(self.hwnd, target);
            }
        }
        let record = format!(
            concat!(
                "{{\"schema\":1,\"paint_diagnostics_enabled\":{},\"pid\":{},\"hwnd\":{},\"sample\":[{},{}],\"before\":{},",
                "\"client_ok\":{},\"client\":[{},{}],\"update_pending\":{},\"update_rect\":[{},{},{},{}],",
                "\"after_flush\":{{\"flush\":{},\"cpu_rgb\":{},\"bitmap_read\":{},\"bitmap\":[{},{}],",
                "\"bitmap_matches\":{},\"source_rgb\":{},\"target_rgb\":{},",
                "\"source_clip_type\":{},\"source_clip\":[{},{},{},{}],",
                "\"target_clip_type\":{},\"target_clip\":[{},{},{},{}]}}}}\n"
            ),
            events.paint_diagnostics,
            unsafe { ffi::GetCurrentProcessId() },
            self.hwnd as usize,
            x,
            y,
            before,
            client_ok,
            client.right,
            client.bottom,
            update_pending,
            update.left,
            update.top,
            update.right,
            update.bottom,
            flush,
            rgb(after_cpu),
            bitmap_read,
            bitmap.width,
            bitmap.height,
            selected == self.surface.bitmap,
            rgb(source),
            rgb(target_rgb),
            source_clip_type,
            source_clip.left,
            source_clip.top,
            source_clip.right,
            source_clip.bottom,
            target_clip_type,
            target_clip.left,
            target_clip.top,
            target_clip.right,
            target_clip.bottom
        );
        if record.len() > 8192 {
            return 0;
        }
        let file = unsafe { ffi::CreateFileW(path.as_ptr(), 0x40000000, 1, NULL, 1, 0x80, NULL) };
        if file == -1isize as Handle {
            return 0;
        }
        let mut written = 0;
        let ok = unsafe {
            ffi::WriteFile(
                file,
                record.as_ptr().cast(),
                record.len() as u32,
                &mut written,
                NULL,
            )
        };
        let closed = unsafe { ffi::CloseHandle(file) };
        isize::from(ok != 0 && written as usize == record.len() && closed != 0)
    }

    fn trace_frame(&self, event: &str, target: Handle, paint: &ffi::Rect, blit: i32) {
        if !self.native_trace.enabled() {
            return;
        }
        let mut client = ffi::Rect::default();
        let mut source_clip = ffi::Rect::default();
        let mut target_clip = ffi::Rect::default();
        let mut bitmap: ffi::Bitmap = unsafe { core::mem::zeroed() };
        let (selected, bitmap_read, source_clip_type, target_clip_type) = unsafe {
            ffi::GdiFlush();
            ffi::GetClientRect(self.hwnd, &mut client);
            let selected = ffi::GetCurrentObject(self.surface.dc, 7);
            (
                selected,
                ffi::GetObjectW(
                    selected,
                    core::mem::size_of::<ffi::Bitmap>() as i32,
                    (&mut bitmap as *mut ffi::Bitmap).cast(),
                ),
                ffi::GetClipBox(self.surface.dc, &mut source_clip),
                if target.is_null() {
                    0
                } else {
                    ffi::GetClipBox(target, &mut target_clip)
                },
            )
        };
        let x = client.right - self.fonts.width / 2 - 1;
        let y = self.fonts.height * 3 / 2;
        let rgb = |value: Option<u32>| {
            value.map_or_else(
                || String::from("null"),
                |value| {
                    format!(
                        "[{},{},{}]",
                        value >> 16 & 255,
                        value >> 8 & 255,
                        value & 255
                    )
                },
            )
        };
        let cpu = if x >= 0
            && x < self.surface.width
            && y >= 0
            && y < self.surface.height
            && !self.surface.pixels.is_null()
        {
            Some(unsafe {
                *self
                    .surface
                    .pixels
                    .add((y * self.surface.width + x) as usize)
            })
        } else {
            None
        };
        let dc_pixel = |dc: Handle| {
            if dc.is_null() {
                return None;
            }
            let value = unsafe { ffi::GetPixel(dc, x, y) };
            (value != u32::MAX).then(|| colorref(value))
        };
        let pane = self.panes.get(self.active).map_or_else(
            || String::from("null"),
            |pane| {
                format!(
                    concat!(
                        "{{\"rect\":[{},{},{},{}],\"grid\":[{},{}],",
                        "\"pty\":[{},{}],\"images\":{},\"placements\":{},\"grid_ready\":{}}}"
                    ),
                    pane.rect.x,
                    pane.rect.y,
                    pane.rect.width,
                    pane.rect.height,
                    pane.core.terminal.cols(),
                    pane.core.terminal.rows(),
                    pane.pty_size.cols,
                    pane.pty_size.rows,
                    pane.core.terminal.graphics().image_count(),
                    pane.core.terminal.graphics().placement_count(),
                    pane.grid_ready()
                )
            },
        );
        self.native_trace.record(format_args!(
            concat!(
                "{{\"event\":\"{}\",\"ms\":{},\"client\":[{},{}],\"surface\":[{},{}],",
                "\"bitmap_read\":{},\"bitmap\":[{},{}],\"bitmap_matches\":{},",
                "\"source_clip_type\":{},\"source_clip\":[{},{},{},{}],",
                "\"paint\":[{},{},{},{}],\"target_clip_type\":{},\"target_clip\":[{},{},{},{}],",
                "\"blit\":{},\"sample\":[{},{}],\"cpu_rgb\":{},\"source_rgb\":{},",
                "\"target_rgb\":{},\"pane\":{}}}"
            ),
            event,
            os::milliseconds(),
            client.right,
            client.bottom,
            self.surface.width,
            self.surface.height,
            bitmap_read,
            bitmap.width,
            bitmap.height,
            selected == self.surface.bitmap,
            source_clip_type,
            source_clip.left,
            source_clip.top,
            source_clip.right,
            source_clip.bottom,
            paint.left,
            paint.top,
            paint.right,
            paint.bottom,
            target_clip_type,
            target_clip.left,
            target_clip.top,
            target_clip.right,
            target_clip.bottom,
            blit,
            x,
            y,
            rgb(cpu),
            rgb(dc_pixel(self.surface.dc)),
            rgb(dc_pixel(target)),
            pane
        ));
    }

    fn enqueue(&mut self, index: usize, bytes: &[u8]) {
        if !self.panes[index].core.input.push(bytes) {
            os::print(2, b"ztty: input queue full; paste was rejected\n");
        }
    }
    fn enqueue_encoded(&mut self) {
        self.enqueue_encoded_for(self.active);
    }
    fn enqueue_encoded_for(&mut self, index: usize) {
        let encoded = core::mem::take(&mut self.encoded);
        self.enqueue(index, &encoded);
        self.encoded = encoded;
        self.encoded.clear();
    }
    fn input(&mut self, key: Key, modifiers: Modifiers, event: KeyEvent, text: &str) {
        self.encoded.clear();
        input::encode_key_into(
            key,
            modifiers,
            event,
            text,
            &self.panes[self.active].core.terminal.modes().input(),
            &mut self.encoded,
        );
        // Silent modifier/release events must not undo a scroll or selection
        // action from the same shortcut. Negotiated keyboard events still
        // return to live output when the encoder actually sends them.
        if self.encoded.is_empty() {
            return;
        }
        self.enqueue_encoded();
        self.panes[self.active].history = 0;
        self.panes[self.active].selection = None;
        self.reset_cursor();
    }
    fn reset_cursor(&mut self) {
        self.panes[self.active].cursor_on = true;
        self.panes[self.active].blink_deadline =
            os::milliseconds() + self.appearance.cursor_blink_interval;
        self.panes[self.active].dirty = true;
    }
    fn cell_at(&self, index: usize, x: i32, y: i32) -> (usize, usize) {
        (
            (((x - self.panes[index].rect.x - self.appearance.padding).max(0) / self.fonts.width)
                as usize)
                .min(self.panes[index].core.terminal.cols() - 1),
            (((y - self.panes[index].rect.y - self.appearance.padding).max(0) / self.fonts.height)
                as usize)
                .min(self.panes[index].core.terminal.rows() - 1),
        )
    }
    fn resize(&mut self) {
        self.resize_with_fonts(None);
    }
    fn resize_with_fonts(&mut self, candidate: Option<Fonts>) {
        if let Some(fonts) = candidate {
            self.pending_fonts = Some(fonts);
        }
        if let Err(error) = self.arrange(self.layout.clone()) {
            self.message(error);
        }
    }

    fn font_action(&mut self, action: &str) -> Result<(), &'static str> {
        // Repeated shortcuts build on the requested font even while PTY
        // compensation is pending; arrange commits font and all grids together.
        let current = self.pending_fonts.as_ref().unwrap_or(&self.fonts);
        let description = if action == "font-reset" {
            self.appearance.font.clone()
        } else {
            let size = if action == "font-increase" {
                current.size.saturating_add(1)
            } else {
                current.size.saturating_sub(1)
            }
            .clamp(5, 96);
            if size == current.size {
                return Ok(());
            }
            let mut description = String::new();
            for (index, part) in current.description.split(':').enumerate() {
                if index != 0 && part.starts_with("size=") {
                    continue;
                }
                if index != 0 {
                    description.push(':');
                }
                description.push_str(part);
            }
            format!("{description}:size={size}")
        };
        let candidate = Fonts::new(self.surface.dc, &description, unsafe {
            ffi::GetDpiForWindow(self.hwnd)
        })?;
        self.resize_with_fonts(Some(candidate));
        Ok(())
    }

    fn grid_size(cols: usize, rows: usize, fonts: &Fonts) -> zpty::Size {
        zpty::Size {
            cols: cols as u16,
            rows: rows as u16,
            pixel_width: (cols as i32 * fonts.width).min(u16::MAX as i32) as u16,
            pixel_height: (rows as i32 * fonts.height).min(u16::MAX as i32) as u16,
        }
    }

    fn size_in(rect: Rect, fonts: &Fonts, padding: i32) -> zpty::Size {
        let cols =
            ((rect.width - 2 * padding).max(fonts.width) / fonts.width).clamp(1, 1024) as usize;
        let rows =
            ((rect.height - 2 * padding).max(fonts.height) / fonts.height).clamp(1, 1024) as usize;
        Self::grid_size(cols, rows, fonts)
    }

    fn preserve_held_frames(&mut self) -> Result<(), &'static str> {
        let now = os::milliseconds();
        for pane in &mut self.panes {
            if pane.core.terminal.modes().synchronized_output {
                let start = *pane.sync_since.get_or_insert(now);
                if now.saturating_sub(start) >= 150 {
                    pane.core.terminal.force_end_sync();
                    pane.sync_since = None;
                }
            }
            if !pane.core.terminal.modes().synchronized_output {
                pane.held_frame = None;
            }
        }
        let used: usize = self
            .panes
            .iter()
            .filter_map(|pane| pane.held_frame.as_ref())
            .map(session_frame::Snapshot::pixel_count)
            .sum();
        let mut remaining = MAX_SURFACE_PIXELS.saturating_sub(used);
        for pane in &mut self.panes {
            if pane.visible
                && pane.core.terminal.modes().synchronized_output
                && pane.held_frame.is_none()
            {
                let frame = self.surface.snapshot(pane.rect, remaining)?;
                remaining -= frame.pixel_count();
                pane.held_frame = Some(frame);
            }
        }
        Ok(())
    }

    /// Coordinate all live PTYs before committing any terminal grid/view/font.
    /// Failed compensation is tracked by pty_size; that pane's output is held
    /// in zpty's bounded queue until a retry restores a matching parser grid.
    fn arrange(&mut self, layout: Layout) -> Result<(), &'static str> {
        self.resize_pending = true;
        self.resize_retry_at = os::milliseconds() + 250;
        let mut client = ffi::Rect::default();
        if unsafe { ffi::GetClientRect(self.hwnd, &mut client) } == 0 {
            return Err("cannot query the Windows workspace dimensions");
        }
        let width = client.right.max(1);
        let height = client.bottom.max(1);
        self.trace_frame("arrange-start", NULL, &ffi::Rect::default(), 0);
        let changed = self.surface.width != width || self.surface.height != height;
        let placements = layout.placements(width, height);
        let geometry_changed = changed
            || self.pending_fonts.is_some()
            || self.panes.iter().any(|pane| {
                let next = placements.iter().find(|(id, _)| *id == pane.id);
                pane.visible != next.is_some() || next.is_some_and(|(_, rect)| pane.rect != *rect)
            });
        if geometry_changed || self.full_redraw {
            // Do this before replacing the DIB or changing pane coordinates.
            // Repeated resizes keep the original committed snapshot.
            self.preserve_held_frames()?;
        }
        let previous_surface = [self.surface.width, self.surface.height];
        if !self.surface.resize(width, height) {
            return Err("cannot allocate the Windows workspace surface");
        }
        if changed && self.native_snapshot_path.is_some() {
            // Include the first DIB allocation, but not no-ops or failed sizes.
            // Later PTY coordination can still reject this layout.
            self.surface_resize = Some(SurfaceResizeRecord {
                count: self
                    .surface_resize
                    .as_ref()
                    .map_or(1, |resize| resize.count.saturating_add(1)),
                from: previous_surface,
                to: [self.surface.width, self.surface.height],
            });
        }
        self.full_redraw |= changed;
        let fonts = self.pending_fonts.as_ref().unwrap_or(&self.fonts);
        let mut plan = Vec::with_capacity(self.panes.len());
        for pane in &self.panes {
            let rect = placements
                .iter()
                .find(|(id, _)| *id == pane.id)
                .map(|(_, rect)| *rect);
            let size = rect.map_or_else(
                || Self::grid_size(pane.core.terminal.cols(), pane.core.terminal.rows(), fonts),
                |rect| Self::size_in(rect, fonts, self.appearance.padding),
            );
            let rollback = Self::grid_size(
                pane.core.terminal.cols(),
                pane.core.terminal.rows(),
                &self.fonts,
            );
            plan.push((rect, size, rollback));
        }
        let mut sizes: Vec<session_resize::Plan> = self
            .panes
            .iter()
            .zip(&plan)
            .map(|(pane, (_, desired, committed))| session_resize::Plan {
                actual: pane.pty_size,
                desired: *desired,
                committed: *committed,
            })
            .collect();
        let result =
            session_resize::coordinate(&mut sizes, self.pending_fonts.is_some(), |index, size| {
                self.panes[index].core.eof() || self.panes[index].core.pty.resize(size).is_ok()
            });
        for (pane, size) in self.panes.iter_mut().zip(sizes) {
            pane.pty_size = size.actual;
        }
        if let Err(error) = result {
            return Err(if error.rollback_failed {
                "workspace PTY rollback incomplete; affected output paused for retry"
            } else {
                "workspace PTY resize failed; previous layout retained and retry scheduled"
            });
        }
        let old_focus = self.panes.get(self.active).map(|pane| pane.id);
        if let Some(fonts) = self.pending_fonts.take() {
            self.fonts = fonts;
        }
        for (pane, (rect, size, _)) in self.panes.iter_mut().zip(plan) {
            let grid_changed = pane.core.terminal.cols() != usize::from(size.cols)
                || pane.core.terminal.rows() != usize::from(size.rows);
            pane.core
                .terminal
                .set_cell_size(self.fonts.width as u32, self.fonts.height as u32);
            if grid_changed {
                let mut anchor = ztty::selection::ViewAnchor::capture(
                    &pane.core.terminal,
                    pane.history,
                    pane.selection
                        .map(|selection| (selection.start, selection.end)),
                );
                pane.core.terminal.resize_anchored(
                    usize::from(size.cols),
                    usize::from(size.rows),
                    anchor.anchors_mut(),
                );
                let restored = anchor.restore(&pane.core.terminal);
                pane.history = restored.history;
                pane.selection = restored
                    .selection
                    .map(|(start, end)| Selection { start, end });
                pane.dragging = false;
            }
            pane.pty_size = size;
            pane.history = pane.history.min(pane.core.terminal.history_len());
            pane.visible = rect.is_some();
            if let Some(rect) = rect {
                pane.rect = rect;
            }
            pane.dirty = true;
        }
        self.layout = layout;
        self.active = self
            .panes
            .iter()
            .position(|pane| pane.id == self.layout.focused)
            .unwrap_or(0);
        self.resize_pending = false;
        self.full_redraw |= geometry_changed;
        self.caption_dirty = true;
        if old_focus != self.panes.get(self.active).map(|pane| pane.id) {
            if let Some(old) =
                old_focus.and_then(|id| self.panes.iter().position(|pane| pane.id == id))
            {
                let bytes =
                    input::encode_focus(false, &self.panes[old].core.terminal.modes().input());
                self.panes[old].core.input.push(&bytes);
            }
            for (held, intercepted) in self.held.iter_mut().zip(&mut self.intercepted) {
                if held.take().is_some() {
                    *intercepted = true;
                }
            }
            self.pending_key = None;
            self.surrogate = 0;
            self.suppress_char = true;
            self.cancel_mouse_capture(true);
            let bytes = input::encode_focus(
                self.focused,
                &self.panes[self.active].core.terminal.modes().input(),
            );
            self.panes[self.active].core.input.push(&bytes);
            self.reset_cursor();
        }
        // Origin, cell metrics or viewport can change without cursor movement.
        self.ime_position();
        unsafe {
            ffi::InvalidateRect(self.hwnd, ptr::null(), 0);
        }
        self.trace_frame("arrange-commit", NULL, &ffi::Rect::default(), 0);
        Ok(())
    }

    fn message(&mut self, message: &str) {
        os::print(2, b"ztty: ");
        os::print(2, message.as_bytes());
        os::print(2, b"\n");
        self.notice = Some((String::from(message), os::milliseconds() + 3000));
        self.caption_dirty = true;
    }

    fn related(&mut self, axis: Option<Axis>) -> Result<(), &'static str> {
        let id = self.next_id;
        let next = id.checked_add(1).ok_or("workspace session IDs exhausted")?;
        let mut layout = self.layout.clone();
        if let Some(axis) = axis {
            layout.split(id, axis)?;
        } else {
            layout.add_tab(id)?;
        }
        let placements = layout.placements(self.surface.width, self.surface.height);
        let rect = placements
            .iter()
            .find(|(pane, _)| *pane == id)
            .map(|(_, rect)| *rect)
            .ok_or("new pane has no placement")?;
        let size = Self::size_in(rect, &self.fonts, self.appearance.padding);
        let program = os::resolve(&self.launch.shell)?;
        let arguments: Vec<u16> = self.launch.shell.to_string_lossy().encode_utf16().collect();
        let pty = zpty::Pty::spawn_utf16(
            &program,
            &[&arguments],
            Some(&self.launch.environment),
            size,
        )
        .map_err(|_| "cannot start the new Windows pseudoconsole")?;
        let mut terminal = Terminal::with_theme(
            usize::from(size.cols),
            usize::from(size.rows),
            self.launch.history,
            self.appearance.theme,
        );
        terminal.set_cell_size(self.fonts.width as u32, self.fonts.height as u32);
        let mut pane = Pane::new(id, pty, terminal, size, rect, &self.wake)?;
        pane.visible = false;
        self.panes.push(pane);
        if let Err(error) = self.arrange(layout) {
            // Only this newly created PTY is discarded. Existing identities stay.
            self.panes.pop();
            return Err(error);
        }
        self.next_id = next;
        Ok(())
    }

    fn close_pane(&mut self, index: usize) {
        if index >= self.panes.len() {
            return;
        }
        let old_focus = self.panes.get(self.active).map(|pane| pane.id);
        let id = self.panes[index].id;
        self.last_status = self.panes[index]
            .core
            .pty
            .status()
            .and_then(|status| status.code())
            .unwrap_or(0);
        self.layout.remove(id);
        let pane = self.panes.remove(index);
        if pane.dragging || self.mouse_capture.is_some_and(|(owner, _)| owner == id) {
            self.cancelled_mouse_buttons |= u8::from(pane.dragging);
            self.cancel_mouse_capture(true);
        }
        drop(pane); // Waits cancel/join before its PTY closes its own process tree.
        if self.panes.is_empty() {
            self.active = 0;
            self.running = false;
            return;
        }
        self.active = self
            .panes
            .iter()
            .position(|pane| pane.id == self.layout.focused)
            .unwrap_or(0);
        let focus_changed = old_focus != Some(self.panes[self.active].id);
        if focus_changed {
            self.pending_key = None;
            self.surrogate = 0;
            self.suppress_char = true;
            for (held, intercepted) in self.held.iter_mut().zip(&mut self.intercepted) {
                if held.take().is_some() {
                    *intercepted = true;
                }
            }
            let bytes = input::encode_focus(
                self.focused,
                &self.panes[self.active].core.terminal.modes().input(),
            );
            self.panes[self.active].core.input.push(&bytes);
        }
        self.resize_with_fonts(None);
        if focus_changed {
            self.reset_cursor();
        }
        self.ime_position();
    }

    fn action(&mut self, action: &str) -> bool {
        let result = match action {
            "new-tab" => self.related(None),
            "split-right" => self.related(Some(Axis::Vertical)),
            "split-down" => self.related(Some(Axis::Horizontal)),
            "close" => {
                self.close_pane(self.active);
                Ok(())
            }
            "copy" => {
                self.copy();
                Ok(())
            }
            "paste" => {
                self.paste();
                Ok(())
            }
            "font-increase" | "font-decrease" | "font-reset" => self.font_action(action),
            "scroll-up" | "scroll-down" => {
                let pane = &mut self.panes[self.active];
                let step = pane.core.terminal.rows().saturating_sub(1).max(1);
                pane.history = if action == "scroll-up" {
                    pane.history
                        .saturating_add(step)
                        .min(pane.core.terminal.history_len())
                } else {
                    pane.history.saturating_sub(step)
                };
                pane.selection = None;
                pane.dirty = true;
                Ok(())
            }
            "next-tab" | "previous-tab" | "next-pane" | "previous-pane" | "swap-next"
            | "swap-previous" | "maximize" | "grow" | "shrink" => {
                let mut layout = self.layout.clone();
                match action {
                    "next-tab" => layout.cycle_tab(true),
                    "previous-tab" => layout.cycle_tab(false),
                    "next-pane" => layout.cycle_pane(true, false),
                    "previous-pane" => layout.cycle_pane(false, false),
                    "swap-next" => layout.cycle_pane(true, true),
                    "swap-previous" => layout.cycle_pane(false, true),
                    "maximize" => layout.maximized = !layout.maximized,
                    "grow" => {
                        layout.adjust(50);
                    }
                    "shrink" => {
                        layout.adjust(-50);
                    }
                    _ => {}
                }
                self.arrange(layout)
            }
            "none" => Ok(()),
            _ => return false,
        };
        if let Err(error) = result {
            self.message(error);
        }
        true
    }

    fn caption(&mut self, now: u64) {
        if self.notice.as_ref().is_some_and(|(_, until)| now >= *until) {
            self.notice = None;
            self.caption_dirty = true;
        }
        let pane = &mut self.panes[self.active];
        if pane.title != pane.core.terminal.title() {
            pane.title = String::from(pane.core.terminal.title());
            self.caption_dirty = true;
        }
        if !self.caption_dirty {
            return;
        }
        let caption = if let Some((message, _)) = &self.notice {
            format!("ztty — {message}")
        } else {
            let title = if pane.title.is_empty() {
                "ztty"
            } else {
                &pane.title
            };
            if self.layout.tabs.len() > 1 {
                format!(
                    "[{} / {}] {title}",
                    self.layout.active_tab + 1,
                    self.layout.tabs.len()
                )
            } else {
                String::from(title)
            }
        };
        if caption != self.title {
            self.title = caption;
            let title = wide(&self.title);
            unsafe {
                ffi::SetWindowTextW(self.hwnd, title.as_ptr());
            }
        }
        self.caption_dirty = false;
    }
    fn tick(&mut self) {
        if self.panes.is_empty() {
            self.running = false;
            return;
        }
        let now = os::milliseconds();
        if self.resize_pending && now >= self.resize_retry_at {
            let _ = self.arrange(self.layout.clone());
        }
        let mut buffer = [0u8; 65536];
        let count = self.panes.len();
        // At most 1 MiB per tick, rotating the first pane so a busy hidden tab
        // cannot starve foreground input or another PTY's responses.
        let mut data = 0;
        let first = self.pump_cursor;
        for offset in 0..count {
            let index = (first + offset) % count;
            let pane = &mut self.panes[index];
            let _ = pane.core.flush_input();
            if pane.grid_ready() {
                for _ in 0..(16 / count).max(1) {
                    match pane.core.read_once(&mut buffer, false) {
                        Ok(ReadStep::Data {
                            history_scrolled,
                            history_after,
                            ..
                        }) => {
                            if pane.history != 0 {
                                pane.history = pane
                                    .history
                                    .saturating_add(history_scrolled)
                                    .min(history_after);
                            }
                            pane.dirty = true;
                            data += 1;
                        }
                        Ok(ReadStep::Pending | ReadStep::Eof) | Err(_) => break,
                    }
                    if data >= 16 {
                        break;
                    }
                }
            }
            if !pane.core.queue_responses() {
                os::print(
                    2,
                    b"ztty: input queue full; terminal replies were rejected\n",
                );
            }
            let _ = pane.core.flush_input();
            let _ = pane.core.pty.reap();
            if pane.grid_ready() {
                pane.output_wait.rearm();
            }
            self.pump_cursor = (index + 1) % count;
            if data >= 16 {
                break;
            }
        }
        self.pump_again = data >= 16;
        for index in (0..self.panes.len()).rev() {
            if self.panes[index].core.eof() || self.panes[index].core.pty.status().is_some() {
                self.close_pane(index);
            }
        }
        if self.panes.is_empty() {
            return;
        }
        let mut position_ime = false;
        for (index, pane) in self.panes.iter_mut().enumerate() {
            let cursor = pane.core.terminal.cursor();
            if cursor != pane.last_cursor {
                pane.last_cursor = cursor;
                pane.cursor_on = true;
                pane.blink_deadline = now + self.appearance.cursor_blink_interval;
                pane.dirty = true;
                position_ime |= index == self.active;
            }
            if pane.core.terminal.modes().synchronized_output {
                let start = *pane.sync_since.get_or_insert(now);
                if now.saturating_sub(start) >= 150 {
                    pane.core.terminal.force_end_sync();
                    pane.sync_since = None;
                }
            } else {
                pane.sync_since = None;
            }
            if !pane.core.terminal.modes().synchronized_output {
                pane.held_frame = None;
            }
            if pane.visible
                && index == self.active
                && self.focused
                && pane
                    .core
                    .terminal
                    .modes()
                    .cursor_blink
                    .unwrap_or(self.appearance.cursor_blink)
                && now >= pane.blink_deadline
            {
                pane.cursor_on = !pane.cursor_on;
                pane.blink_deadline = now + self.appearance.cursor_blink_interval;
                pane.dirty = true;
            }
            let revision = pane.core.terminal.graphics().revision();
            if revision != pane.graphics_revision {
                pane.graphics_revision = revision;
                pane.dirty = true;
            }
        }
        if position_ime {
            self.ime_position();
        }
        self.caption(now);
        if self.full_redraw
            || self.panes.iter().any(|pane| {
                pane.visible
                    && !pane.core.terminal.modes().synchronized_output
                    && (pane.dirty || pane.core.terminal.dirty())
            })
        {
            unsafe {
                ffi::InvalidateRect(self.hwnd, ptr::null(), 0);
            }
        }
    }

    fn wait_timeout(&self) -> u32 {
        if self.pump_again {
            return 0;
        }
        let now = os::milliseconds();
        let mut timeout = 1000;
        for (index, pane) in self.panes.iter().enumerate() {
            if pane.visible
                && index == self.active
                && self.focused
                && pane
                    .core
                    .terminal
                    .modes()
                    .cursor_blink
                    .unwrap_or(self.appearance.cursor_blink)
            {
                timeout = timeout.min(pane.blink_deadline.saturating_sub(now).min(1000) as u32);
            }
            if pane.core.input.pending() {
                timeout = timeout.min(8);
            }
            if let Some(start) = pane.sync_since {
                timeout = timeout.min((start + 150).saturating_sub(now).min(1000) as u32);
            }
        }
        if self.resize_pending {
            timeout = timeout.min(self.resize_retry_at.saturating_sub(now).min(1000) as u32);
        }
        if let Some((_, until)) = self.notice.as_ref() {
            timeout = timeout.min(until.saturating_sub(now).min(1000) as u32);
        }
        timeout
    }
    fn images(&mut self, index: usize, range: core::ops::Range<i64>) {
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
    fn draw_pane(&mut self, index: usize) {
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
        for row in 0..self.panes[index].core.terminal.rows() {
            for col in 0..self.panes[index].core.terminal.cols() {
                let cell =
                    self.panes[index]
                        .core
                        .terminal
                        .view_cell(col, row, self.panes[index].history);
                if cell.width == 0 || cell.flags & zterm::HIDDEN != 0 {
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
    fn paint(&mut self, events: &NativeEvents) {
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
    fn ime_position(&self) {
        let context = unsafe { ffi::ImmGetContext(self.hwnd) };
        if context.is_null() {
            return;
        }
        let (col, row) = self.panes[self.active].core.terminal.cursor();
        let form = ffi::CompositionForm {
            style: 2,
            position: ffi::Point {
                x: self.panes[self.active].rect.x
                    + self.appearance.padding
                    + col as i32 * self.fonts.width,
                y: self.panes[self.active].rect.y
                    + self.appearance.padding
                    + (row + 1) as i32 * self.fonts.height,
            },
            area: ffi::Rect::default(),
        };
        unsafe {
            ffi::ImmSetCompositionWindow(context, &form);
            ffi::ImmReleaseContext(self.hwnd, context);
        }
    }
    fn copy(&self) {
        let Some(selection) = self.panes[self.active].selection else {
            return;
        };
        let text = wide(&selection.text(
            &self.panes[self.active].core.terminal,
            self.panes[self.active].history,
        ));
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
                        &self.panes[self.active].core.terminal.modes().input(),
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
        self.panes[self.active].history = 0;
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
            let logical = virtual_key(vk as u32, flags);
            if let Some(action) = actions::key_action(
                &self.appearance.bindings,
                binding_symbol(logical),
                binding_state(mods),
            ) && self.action(action)
            {
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
            input::encode_text_into(
                text,
                &self.panes[self.active].core.terminal.modes().input(),
                &mut self.encoded,
            );
            self.enqueue_encoded();
            self.panes[self.active].history = 0;
            self.reset_cursor();
        }
    }
    fn cancel_mouse_capture(&mut self, release: bool) {
        if let Some((_, buttons)) = self.mouse_capture.take() {
            self.cancelled_mouse_buttons |= buttons;
        }
        for pane in &mut self.panes {
            if pane.dragging {
                self.cancelled_mouse_buttons |= 1;
                pane.dragging = false;
            }
        }
        if release {
            unsafe {
                ffi::ReleaseCapture();
            }
        }
    }
    fn mouse(&mut self, message: u32, wparam: usize, lparam: isize) {
        if let Some((button, pressed)) = mouse_button(message) {
            let bit = 1 << button;
            let cancelled = self.cancelled_mouse_buttons & bit != 0;
            self.cancelled_mouse_buttons &= !bit;
            if !pressed && cancelled {
                return;
            }
        }
        let mut point = ffi::Point {
            x: lparam as i16 as i32,
            y: (lparam >> 16) as i16 as i32,
        };
        if message == 0x020a {
            unsafe {
                ffi::ScreenToClient(self.hwnd, &mut point);
            }
        }
        let hit = self.panes.iter().position(|pane| {
            pane.visible
                && point.x >= pane.rect.x
                && point.y >= pane.rect.y
                && point.x < pane.rect.x.saturating_add(pane.rect.width)
                && point.y < pane.rect.y.saturating_add(pane.rect.height)
        });
        let capture = self
            .mouse_capture
            .and_then(|(id, _)| self.panes.iter().position(|pane| pane.id == id));
        let index = if let Some(index) = capture {
            index
        } else if self.panes[self.active].dragging {
            self.active
        } else if let Some(index) = hit {
            index
        } else {
            return;
        };
        if matches!(message, 0x0201 | 0x0204 | 0x0207) && index != self.active {
            let mut layout = self.layout.clone();
            layout.focus(self.panes[index].id);
            if let Err(error) = self.arrange(layout) {
                self.message(error);
                return;
            }
        }
        self.mouse_at(index, message, wparam, lparam);
    }

    fn mouse_at(&mut self, index: usize, message: u32, wparam: usize, lparam: isize) {
        let mut point = ffi::Point {
            x: lparam as i16 as i32,
            y: (lparam >> 16) as i16 as i32,
        };
        if message == 0x020a {
            unsafe {
                ffi::ScreenToClient(self.hwnd, &mut point);
            }
        }
        let (col, row) = self.cell_at(index, point.x, point.y);
        let mods = modifiers();
        let app_tracking = (self.panes[index].core.terminal.modes().mouse_tracking != 0
            && !mods.contains(Modifiers::SHIFT))
            || self
                .mouse_capture
                .is_some_and(|(owner, _)| owner == self.panes[index].id);
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
                button: if wparam & 1 != 0 && self.cancelled_mouse_buttons & 1 == 0 {
                    Some(0)
                } else if wparam & 0x10 != 0 && self.cancelled_mouse_buttons & 2 == 0 {
                    Some(1)
                } else if wparam & 2 != 0 && self.cancelled_mouse_buttons & 4 == 0 {
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
            if let Some((button, true)) = mouse_button(message) {
                let capture = self.mouse_capture.get_or_insert((self.panes[index].id, 0));
                capture.1 |= 1 << button;
                unsafe {
                    ffi::SetCapture(self.hwnd);
                }
            }
            if let Some(event) = event {
                self.encoded.clear();
                input::encode_mouse_into(
                    event,
                    mods,
                    &self.panes[index].core.terminal.modes().input(),
                    &mut self.encoded,
                );
                self.enqueue_encoded_for(index);
            }
            if let Some((button, false)) = mouse_button(message)
                && let Some((_, buttons)) = self.mouse_capture.as_mut()
            {
                *buttons &= !(1 << button);
                if *buttons == 0 {
                    self.mouse_capture = None;
                    unsafe {
                        ffi::ReleaseCapture();
                    }
                }
            }
            return;
        }
        match message {
            0x0201 => {
                if mods.contains(Modifiers::CTRL) {
                    self.open_link(col, row);
                    return;
                }
                self.panes[index].selection = Some(Selection {
                    start: (col, row),
                    end: (col, row),
                });
                self.panes[index].dragging = true;
                self.panes[index].dirty = true;
                unsafe {
                    ffi::SetCapture(self.hwnd);
                }
            }
            0x0200 if self.panes[index].dragging => {
                if let Some(selection) = self.panes[index].selection.as_mut() {
                    selection.end = (col, row);
                    self.panes[index].dirty = true;
                }
            }
            0x0202 if self.panes[index].dragging => {
                self.panes[index].dragging = false;
                unsafe {
                    ffi::ReleaseCapture();
                }
            }
            0x0207 => self.paste(),
            0x020a => {
                let delta = (wparam >> 16) as i16 as i32;
                let lines = (delta.unsigned_abs() as usize / 120).max(1) * 3;
                self.panes[index].history = if delta > 0 {
                    self.panes[index]
                        .history
                        .saturating_add(lines)
                        .min(self.panes[index].core.terminal.history_len())
                } else {
                    self.panes[index].history.saturating_sub(lines)
                };
                self.panes[index].selection = None;
                self.panes[index].dirty = true;
            }
            _ => {}
        }
    }
    fn open_link(&self, col: usize, row: usize) {
        let id = self.panes[self.active]
            .core
            .terminal
            .view_cell(col, row, self.panes[self.active].history)
            .link_id;
        let Some(uri) = self.panes[self.active].core.terminal.hyperlink(id) else {
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

fn mouse_button(message: u32) -> Option<(u8, bool)> {
    match message {
        0x0201 => Some((0, true)),
        0x0202 => Some((0, false)),
        0x0204 => Some((2, true)),
        0x0205 => Some((2, false)),
        0x0207 => Some((1, true)),
        0x0208 => Some((1, false)),
        _ => None,
    }
}

pub(crate) fn action_supported(action: &str) -> bool {
    matches!(
        action,
        "new-tab"
            | "split-right"
            | "split-down"
            | "next-tab"
            | "previous-tab"
            | "next-pane"
            | "previous-pane"
            | "swap-next"
            | "swap-previous"
            | "maximize"
            | "grow"
            | "shrink"
            | "close"
            | "copy"
            | "paste"
            | "font-increase"
            | "font-decrease"
            | "font-reset"
            | "scroll-up"
            | "scroll-down"
            | "none"
    )
}

fn binding_state(modifiers: Modifiers) -> u32 {
    u32::from(modifiers.contains(Modifiers::SHIFT))
        | (u32::from(modifiers.contains(Modifiers::CTRL)) << 2)
        | (u32::from(modifiers.contains(Modifiers::ALT)) << 3)
        | (u32::from(modifiers.contains(Modifiers::SUPER)) << 6)
}

fn binding_symbol(key: Key) -> u64 {
    match key {
        Key::Character(ch) => u64::from(ch as u32),
        Key::KeypadAdd => 0xffab,
        Key::KeypadSubtract => 0xffad,
        Key::KeypadDigit(0) => 0xffb0,
        Key::Escape => 0xff1b,
        Key::Enter | Key::KeypadEnter => 0xff0d,
        Key::Tab => 0xff09,
        Key::Backspace => 0xff08,
        Key::Left => 0xff51,
        Key::Right => 0xff53,
        Key::Up => 0xff52,
        Key::Down => 0xff54,
        Key::PageUp => 0xff55,
        Key::PageDown => 0xff56,
        Key::Home => 0xff50,
        Key::End => 0xff57,
        Key::Function(number) => 0xffbd + u64::from(number),
        _ => 0,
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
    dispatch: session_dispatch::Dispatch,
    native_trace: NativeTrace,
    native_events: NativeEvents,
}
impl WindowState {
    fn with<R>(&self, operation: impl FnOnce(&mut App) -> R) -> Option<R> {
        self.dispatch
            .with(|| operation(unsafe { &mut *self.app.get() }))
    }
}

unsafe fn default_window_proc(
    state: &WindowState,
    hwnd: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    let previous = state.native_events.enter_default(message);
    let result = unsafe { ffi::DefWindowProcW(hwnd, message, wparam, lparam) };
    state.native_events.exit_default(message, previous);
    result
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
    let event_sequence = state.native_events.received(message);
    if message == WM_NCCALCSIZE {
        // Copy only scalars while the message parameters are valid. No borrow
        // or pointer into them survives the reentrant default procedure.
        let input = if state.native_events.enabled && wparam != 0 {
            unsafe { (lparam as *const ffi::NcCalcSizeParams).as_ref() }.map(NcCalcStage::capture)
        } else {
            None
        };
        let result = unsafe { default_window_proc(state, hwnd, message, wparam, lparam) };
        if wparam != 0
            && let Some(params) = unsafe { (lparam as *mut ffi::NcCalcSizeParams).as_mut() }
        {
            let after_default = input.map(|_| NcCalcStage::capture(params));
            // Keep default frame/client geometry, but preserve no old client
            // pixels: the retained DIB supplies the resized frame.
            params.rects[1] = ffi::Rect::default();
            params.rects[2] = ffi::Rect::default();
            if let (Some(input), Some(after_default)) = (input, after_default) {
                let submitted = NcCalcStage::capture(params);
                state.native_events.update(|record| {
                    record.nccalc_valid_rects = Some(NcCalcEvent {
                        sequence: event_sequence,
                        wparam,
                        input,
                        default_return: result,
                        after_default,
                        submitted_return: WVR_VALIDRECTS,
                        submitted,
                    });
                });
            }
            return WVR_VALIDRECTS;
        }
        return result;
    }
    if state.native_events.enabled
        && message == 0x0047
        && let Some(position) = unsafe { (lparam as *const ffi::WindowPos).as_ref() }
    {
        state.native_events.update(|record| {
            record.changed = PositionEvent {
                sequence: event_sequence,
                flags_in: position.flags,
                flags_submitted: position.flags,
                position: [position.x, position.y, position.width, position.height],
                current_outer: None,
            }
        });
    }
    if message == WM_WINDOWPOSCHANGING {
        // The retained DIB is the source of every resized frame. Do not also
        // preserve and copy back old window pixels across a size change.
        // Keep default frame/minimum-size processing and every other flag.
        let mut current = ffi::Rect::default();
        if let Some(position) = unsafe { (lparam as *mut ffi::WindowPos).as_mut() } {
            let flags_in = position.flags;
            let mut current_outer = None;
            if position.flags & (SWP_NOSIZE | SWP_NOREDRAW) == 0
                && unsafe { ffi::GetWindowRect(hwnd, &mut current) } != 0
            {
                current_outer = Some([current.left, current.top, current.right, current.bottom]);
                if current.right.saturating_sub(current.left) != position.width
                    || current.bottom.saturating_sub(current.top) != position.height
                {
                    position.flags |= SWP_NOCOPYBITS;
                }
            }
            state.native_events.update(|record| {
                if event_sequence >= record.changing.sequence {
                    record.changing = PositionEvent {
                        sequence: event_sequence,
                        flags_in,
                        flags_submitted: position.flags,
                        position: [position.x, position.y, position.width, position.height],
                        current_outer,
                    };
                }
            });
        }
        return unsafe { default_window_proc(state, hwnd, message, wparam, lparam) };
    }
    if message == WM_NATIVE_PAINT_SAMPLE {
        if state.native_events.enabled
            && state.native_events.paint_diagnostics
            && wparam == NATIVE_SNAPSHOT_MAGIC
        {
            state
                .native_events
                .paint_sample
                .set(Some([lparam as u16 as i32, (lparam >> 16) as u16 as i32]));
            return 1;
        }
        return 0;
    }
    if message == WM_NATIVE_SNAPSHOT && wparam == NATIVE_SNAPSHOT_MAGIC {
        let resize_pending = state.dispatch.resize_pending();
        let paint_pending = state.dispatch.paint_pending();
        let deferred_paints = state.dispatch.deferred_paints();
        return state
            .with(|app| {
                app.failure_snapshot(
                    lparam as u16 as i32,
                    (lparam >> 16) as u16 as i32,
                    resize_pending,
                    paint_pending,
                    deferred_paints,
                    &state.native_events,
                )
            })
            .unwrap_or(0);
    }
    if message == WM_SIZE {
        // Record sent WM_SIZE before trying App so a reentrant request remains
        // pending for WM_PAINT or the outer loop when App is already borrowed.
        state.dispatch.resize_event(wparam == 1);
        state.native_events.update(|record| {
            record.size = [
                wparam as u32,
                lparam as u16 as u32,
                (lparam >> 16) as u16 as u32,
            ];
        });
        if state.native_trace.enabled() {
            let mut client = ffi::Rect::default();
            unsafe {
                ffi::GetClientRect(hwnd, &mut client);
            }
            state.native_trace.record(format_args!(
                "{{\"event\":\"wm-size\",\"ms\":{},\"kind\":{},\"message_size\":[{},{}],\"client\":[{},{}]}}",
                os::milliseconds(), wparam, lparam as u16, (lparam >> 16) as u16,
                client.right, client.bottom));
        }
        if wparam != 1 {
            state.with(|app| {
                if !app.panes.is_empty() && state.dispatch.take_resize() {
                    app.resize();
                }
            });
        }
        return 0;
    }
    let handled = state.with(|app| {
        if app.panes.is_empty() && !matches!(message, WM_PAINT | WM_CLOSE | WM_DESTROY | 0x0082) {
            return Some(0);
        }
        match message {
            WM_PAINT => {
                // A sent resize can precede this paint while the outer loop is
                // still draining messages. Commit its latest client size first.
                if state.dispatch.take_resize() && !app.panes.is_empty() {
                    app.resize();
                }
                app.paint(&state.native_events);
            }
            0x0014 => return Some(1),
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
                app.encoded = input::encode_focus(
                    app.focused,
                    &app.panes[app.active].core.terminal.modes().input(),
                );
                app.enqueue_encoded();
                app.reset_cursor();
                if app.focused {
                    app.ime_position();
                } else {
                    app.held.fill(None);
                    app.intercepted.fill(false);
                    app.cancel_mouse_capture(true);
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
            0x001f => app.cancel_mouse_capture(true),
            0x0215 => app.cancel_mouse_capture(false),
            0x02e0 => {
                let dpi = wparam as u16 as u32;
                let current = app.pending_fonts.as_ref().unwrap_or(&app.fonts);
                let candidate = Fonts::new(app.surface.dc, &current.description, dpi).ok();
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
    if message == WM_PAINT && handled.is_none() {
        // Default processing validates the region even though App could not
        // paint. Retain the obligation and re-invalidate outside the guard.
        state.dispatch.defer_paint();
    }
    handled
        .flatten()
        .unwrap_or_else(|| unsafe { default_window_proc(state, hwnd, message, wparam, lparam) })
}

pub fn run(cfg: Config) -> Result<i32, &'static str> {
    unsafe {
        ffi::SetProcessDpiAwarenessContext(-4isize as Handle);
    }
    let native_trace =
        os::environment_value("ZTTY_NATIVE_TRACE").is_some_and(|value| value == [u16::from(b'1')]);
    let paint_diagnostics = os::environment_value("ZTTY_NATIVE_PAINT_DIAGNOSTICS")
        .is_some_and(|value| value == [u16::from(b'1')]);
    let native_snapshot_path = os::environment_value("ZTTY_NATIVE_SNAPSHOT").map(|mut path| {
        path.push(0);
        path
    });
    let native_events = NativeEvents::new(native_snapshot_path.is_some(), paint_diagnostics);
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
    let wake = session_wait::Wake::new()?;
    let state = Box::new(WindowState {
        app: UnsafeCell::new(App {
            hwnd: NULL,
            panes: vec![Pane::new(
                1,
                pty,
                terminal,
                zpty::Size::new(cfg.cols as u16, cfg.rows as u16),
                Rect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                },
                &wake,
            )?],
            active: 0,
            layout: Layout::new(1),
            next_id: 2,
            launch: Launch {
                shell: os::default_shell(),
                environment,
                history: cfg.history,
            },
            wake: Arc::clone(&wake),
            pump_cursor: 0,
            pump_again: false,
            pending_fonts: None,
            resize_pending: false,
            resize_retry_at: 0,
            full_redraw: true,
            caption_dirty: true,
            notice: None,
            last_status: 0,
            appearance: cfg.appearance,
            surface,
            fonts,
            encoded: Vec::with_capacity(128),
            running: true,
            held: [None; 256],
            intercepted: [false; 256],
            pending_key: None,
            suppress_char: false,
            surrogate: 0,
            mouse_capture: None,
            cancelled_mouse_buttons: 0,
            title: String::new(),
            focused: false,
            native_trace: NativeTrace::new(native_trace),
            native_snapshot_path,
            surface_resize: None,
            last_paint: PaintRecord::default(),
        }),
        dispatch: session_dispatch::Dispatch::default(),
        native_trace: NativeTrace::new(native_trace),
        native_events,
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
    // Keep standard Win32 decorations while isolating the retained GDI client
    // from the separate DWM nonclient rendering path.
    let nonclient_policy = 1i32; // DWMNCRP_DISABLED
    unsafe {
        ffi::DwmSetWindowAttribute(hwnd, 20, (&dark as *const i32).cast(), 4);
        if ffi::DwmSetWindowAttribute(hwnd, 2, (&nonclient_policy as *const i32).cast(), 4) < 0 {
            os::print(2, b"ztty: native window frame policy unavailable\n");
        }
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
                if state.dispatch.take_resize() && !app.panes.is_empty() {
                    app.resize();
                }
                app.tick();
                app.running
            })
            .unwrap_or(false);
        if !running {
            break;
        }
        // Consume before entering Win32; a reentrant paint creates a fresh
        // obligation for the next turn. Never call paint through an App borrow.
        if state.dispatch.take_paint() && unsafe { ffi::InvalidateRect(hwnd, ptr::null(), 0) } == 0
        {
            state.dispatch.retry_paint();
        }
        // The wake mask includes existing paint work; PeekMessage dispatches it.
        let timeout = state.with(|app| app.wait_timeout()).unwrap_or(8);
        let timeout = if state.dispatch.resize_pending() || state.dispatch.paint_pending() {
            0
        } else {
            timeout
        };
        if let Err(error) = wake.wait(timeout) {
            state.with(|app| app.message(error));
            break;
        }
    }
    let status = state
        .with(|app| {
            if let Some(pane) = app.panes.get_mut(app.active) {
                let _ = pane.core.pty.wait_timeout(20);
                pane.core
                    .pty
                    .status()
                    .and_then(|status| status.code())
                    .unwrap_or(app.last_status)
            } else {
                app.last_status
            }
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
    #[derive(Clone, Copy)]
    pub struct WindowPos {
        pub hwnd: Handle,
        pub insert_after: Handle,
        pub x: i32,
        pub y: i32,
        pub width: i32,
        pub height: i32,
        pub flags: u32,
    }
    #[repr(C)]
    pub struct NcCalcSizeParams {
        pub rects: [Rect; 3],
        pub position: *mut WindowPos,
    }
    #[cfg(target_pointer_width = "64")]
    const _: () = {
        assert!(core::mem::size_of::<NcCalcSizeParams>() == 56);
        assert!(core::mem::offset_of!(NcCalcSizeParams, position) == 48);
    };
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
    pub struct Bitmap {
        pub kind: i32,
        pub width: i32,
        pub height: i32,
        pub width_bytes: i32,
        pub planes: u16,
        pub bits_per_pixel: u16,
        pub bits: *mut c_void,
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
        pub fn GetDpiForWindow(hwnd: Handle) -> u32;
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
        pub fn GetClientRect(hwnd: Handle, rect: *mut Rect) -> i32;
        pub fn GetWindowRect(hwnd: Handle, rect: *mut Rect) -> i32;
        pub fn GetUpdateRect(hwnd: Handle, rect: *mut Rect, erase: i32) -> i32;
        pub fn GetDC(hwnd: Handle) -> Handle;
        pub fn ReleaseDC(hwnd: Handle, dc: Handle) -> i32;
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
        pub fn GetCurrentObject(dc: Handle, kind: u32) -> Handle;
        pub fn GetObjectW(object: Handle, bytes: i32, result: *mut c_void) -> i32;
        pub fn GetPixel(dc: Handle, x: i32, y: i32) -> u32;
        pub fn GetClipBox(dc: Handle, rect: *mut Rect) -> i32;
        pub fn GetDCOrgEx(dc: Handle, point: *mut Point) -> i32;
        pub fn SaveDC(dc: Handle) -> i32;
        pub fn RestoreDC(dc: Handle, saved: i32) -> i32;
        pub fn IntersectClipRect(dc: Handle, left: i32, top: i32, right: i32, bottom: i32) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GetModuleHandleW(name: *const u16) -> Handle;
        pub fn GetCurrentProcessId() -> u32;
        pub fn ReadProcessMemory(
            process: Handle,
            source: *const c_void,
            target: *mut c_void,
            size: usize,
            read: *mut usize,
        ) -> i32;
        pub fn CreateFileW(
            path: *const u16,
            access: u32,
            share: u32,
            security: *const c_void,
            disposition: u32,
            attributes: u32,
            template: Handle,
        ) -> Handle;
        pub fn WriteFile(
            file: Handle,
            data: *const c_void,
            size: u32,
            written: *mut u32,
            overlapped: Handle,
        ) -> i32;
        pub fn CloseHandle(handle: Handle) -> i32;
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
