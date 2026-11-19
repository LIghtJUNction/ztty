//! Native message and paint diagnostics.

use super::*;

#[derive(Default)]
pub(super) struct PaintRecord {
    pub(super) sequence: u64,
    pub(super) surface: [i32; 2],
    pub(super) region: [i32; 4],
    pub(super) blit: i32,
    pub(super) start_event_sequence: u64,
    pub(super) end_event_sequence: u64,
    pub(super) default_message: u32,
    pub(super) default_depth: u32,
    pub(super) target: Option<PaintTarget>,
    pub(super) target_after_end: Option<PaintAfterEnd>,
}

pub(super) struct PaintTarget {
    pub(super) sample: [i32; 2],
    pub(super) cpu_before_blit: Option<u32>,
    pub(super) flush_after_blit: i32,
    pub(super) target_after_flush: Option<u32>,
    pub(super) source_after_target_read: Option<u32>,
    pub(super) clip_type: i32,
    pub(super) clip: [i32; 4],
    pub(super) origin_ok: bool,
    pub(super) origin: [i32; 2],
}

pub(super) struct PaintAfterEnd {
    pub(super) sample: [i32; 2],
    pub(super) sample_end_event_sequence: u64,
    pub(super) end_paint_return: i32,
    pub(super) client_dc_ok: bool,
    pub(super) target_rgb: Option<u32>,
    pub(super) release_dc_return: Option<i32>,
}

pub(super) struct SurfaceResizeRecord {
    pub(super) count: u64,
    pub(super) from: [i32; 2],
    pub(super) to: [i32; 2],
}

pub(super) const POSITION_MESSAGES: [u32; 6] = [
    WM_WINDOWPOSCHANGING,
    0x0047,
    0x0088,
    0x0085,
    0x0014,
    WM_SIZE,
];

#[derive(Clone, Copy, Default)]
pub(super) struct MessagePhase {
    pub(super) received: u64,
    pub(super) default_enter: u64,
    pub(super) default_exit: u64,
}

#[derive(Clone, Copy, Default)]
pub(super) struct PositionEvent {
    pub(super) sequence: u64,
    pub(super) flags_in: u32,
    pub(super) flags_submitted: u32,
    pub(super) position: [i32; 4],
    pub(super) current_outer: Option<[i32; 4]>,
}
impl PositionEvent {
    pub(super) fn json(self) -> String {
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
pub(super) struct NcCalcStage {
    pub(super) rects: [[i32; 4]; 3],
    pub(super) position: Option<ffi::WindowPos>,
}
impl NcCalcStage {
    pub(super) fn capture(params: &ffi::NcCalcSizeParams) -> Self {
        Self {
            rects: params
                .rects
                .each_ref()
                .map(|rect| [rect.left, rect.top, rect.right, rect.bottom]),
            position: unsafe { params.position.as_ref() }.copied(),
        }
    }
    pub(super) fn json(self) -> String {
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
pub(super) struct NcCalcEvent {
    pub(super) sequence: u64,
    pub(super) wparam: usize,
    pub(super) input: NcCalcStage,
    pub(super) default_return: isize,
    pub(super) after_default: NcCalcStage,
    pub(super) submitted_return: isize,
    pub(super) submitted: NcCalcStage,
}
impl NcCalcEvent {
    pub(super) fn json(self) -> String {
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
pub(super) struct NativeEventRecord {
    pub(super) messages: [MessagePhase; 6],
    pub(super) changing: PositionEvent,
    pub(super) changed: PositionEvent,
    pub(super) size: [u32; 3],
    pub(super) default_message: u32,
    pub(super) default_depth: u32,
    pub(super) nccalc_valid_rects: Option<NcCalcEvent>,
}

/// Memory-only ordering evidence. Every Cell update finishes before a native
/// call can reenter; formatting happens only for the failure snapshot.
pub(super) struct NativeEvents {
    pub(super) enabled: bool,
    pub(super) paint_diagnostics: bool,
    pub(super) paint_sample: Cell<Option<[i32; 2]>>,
    pub(super) sequence: Cell<u64>,
    pub(super) record: Cell<NativeEventRecord>,
}
impl NativeEvents {
    pub(super) fn new(enabled: bool, paint_diagnostics: bool) -> Self {
        Self {
            enabled,
            paint_diagnostics,
            paint_sample: Cell::new(None),
            sequence: Cell::new(0),
            record: Cell::new(NativeEventRecord::default()),
        }
    }
    pub(super) fn next(&self) -> u64 {
        if !self.enabled {
            return 0;
        }
        let sequence = self.sequence.get().saturating_add(1);
        self.sequence.set(sequence);
        sequence
    }
    pub(super) fn update(&self, update: impl FnOnce(&mut NativeEventRecord)) {
        if self.enabled {
            let mut record = self.record.get();
            update(&mut record);
            self.record.set(record);
        }
    }
    pub(super) fn received(&self, message: u32) -> u64 {
        if !self.enabled {
            return 0;
        }
        let sequence = self.next();
        if let Some(index) = POSITION_MESSAGES.iter().position(|value| *value == message) {
            self.update(|record| record.messages[index].received = sequence);
        }
        sequence
    }
    pub(super) fn enter_default(&self, message: u32) -> u32 {
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
    pub(super) fn exit_default(&self, message: u32, previous: u32) {
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
    pub(super) fn json(&self) -> String {
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

pub(super) struct NativeTrace {
    pub(super) remaining: Cell<u16>,
}
impl NativeTrace {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            remaining: Cell::new(if enabled { 64 } else { 0 }),
        }
    }
    pub(super) fn enabled(&self) -> bool {
        self.remaining.get() != 0
    }
    pub(super) fn record(&self, message: core::fmt::Arguments<'_>) {
        if self.enabled() {
            self.remaining.set(self.remaining.get() - 1);
            os::print(2, format!("{message}\n").as_bytes());
        }
    }
}

impl App {
    /// An explicit diagnostic read of our DIB through the kernel, without asking
    /// GDI to flush its batch. The value may precede queued GDI work.
    pub(super) fn diagnostic_cpu_pixel(&self, x: i32, y: i32) -> Option<u32> {
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

    pub(super) fn failure_snapshot(
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

    pub(super) fn trace_frame(&self, event: &str, target: Handle, paint: &ffi::Rect, blit: i32) {
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
}
