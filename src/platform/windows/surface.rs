//! Retained DIB storage and GDI font resources.

use super::*;

pub(super) struct Surface {
    pub(super) dc: Handle,
    pub(super) bitmap: Handle,
    pub(super) previous: Handle,
    pub(super) pixels: *mut u32,
    pub(super) clip: Rect,
    pub(super) width: i32,
    pub(super) height: i32,
}
impl Surface {
    pub(super) fn new() -> Result<Self, &'static str> {
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
    pub(super) fn resize(&mut self, width: i32, height: i32) -> bool {
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
    pub(super) fn snapshot(
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
    pub(super) fn restore_snapshot(&mut self, snapshot: &session_frame::Snapshot, viewport: Rect) {
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
    pub(super) fn fill(&mut self, x: i32, y: i32, width: i32, height: i32, rgb: u32) {
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
    pub(super) fn image(
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
        let clip = draw.clip;
        let left = dx
            .max(px)
            .max(self.clip.x as i64)
            .max(0)
            .max(clip.map_or(i64::MIN, |c| px + i64::from(c.x)));
        let top = dy
            .max(py)
            .max(self.clip.y as i64)
            .max(0)
            .max(clip.map_or(i64::MIN, |c| py + i64::from(c.y)));
        let right = (dx + i64::from(draw.dest.width))
            .min(px + cols as i64 * i64::from(cell_width))
            .min(i64::from(self.clip.x) + i64::from(self.clip.width))
            .min(self.width as i64)
            .min(clip.map_or(i64::MAX, |c| px + i64::from(c.x) + i64::from(c.width)));
        let bottom = (dy + i64::from(draw.dest.height))
            .min(py + rows as i64 * i64::from(cell_height))
            .min(i64::from(self.clip.y) + i64::from(self.clip.height))
            .min(self.height as i64)
            .min(clip.map_or(i64::MAX, |c| py + i64::from(c.y) + i64::from(c.height)));
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

pub(super) struct Fonts {
    pub(super) normal: Handle,
    pub(super) bold: Handle,
    pub(super) italic: Handle,
    pub(super) bold_italic: Handle,
    pub(super) description: String,
    pub(super) size: u32,
    pub(super) width: i32,
    pub(super) height: i32,
}
impl Fonts {
    pub(super) fn new(dc: Handle, description: &str, dpi: u32) -> Result<Self, &'static str> {
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
    pub(super) fn face(&self, flags: u16) -> Handle {
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
