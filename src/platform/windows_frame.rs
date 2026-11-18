//! A bounded copy of pixels already committed to the native window.
//! Used only while a layout change would overwrite a synchronized pane.
use alloc::vec::Vec;
use ztty::layout::Rect;

pub struct Snapshot {
    width: usize,
    height: usize,
    offset_x: usize,
    offset_y: usize,
    pixels: Vec<u32>,
}

impl Snapshot {
    pub fn capture(
        source: &[u32],
        stride: usize,
        viewport: Rect,
        budget: usize,
    ) -> Result<Self, &'static str> {
        if stride == 0 {
            return Err("committed workspace surface has no stride");
        }
        let height = source.len() / stride;
        let left = i64::from(viewport.x).clamp(0, stride as i64) as usize;
        let top = i64::from(viewport.y).clamp(0, height as i64) as usize;
        let right = (i64::from(viewport.x) + i64::from(viewport.width))
            .clamp(left as i64, stride as i64) as usize;
        let bottom = (i64::from(viewport.y) + i64::from(viewport.height))
            .clamp(top as i64, height as i64) as usize;
        let width = right - left;
        let rows = bottom - top;
        let count = width
            .checked_mul(rows)
            .filter(|count| *count <= budget)
            .ok_or("workspace resize waits for synchronized output to finish")?;
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(count)
            .map_err(|_| "cannot preserve the committed synchronized pane")?;
        for row in top..bottom {
            pixels.extend_from_slice(&source[row * stride + left..row * stride + right]);
        }
        Ok(Self {
            width,
            height: rows,
            offset_x: (left as i64 - i64::from(viewport.x)).max(0) as usize,
            offset_y: (top as i64 - i64::from(viewport.y)).max(0) as usize,
            pixels,
        })
    }

    pub fn pixel_count(&self) -> usize {
        self.pixels.len()
    }

    /// Preserve native pixel values; new exposed space keeps the background.
    /// Both the destination surface and the new pane viewport clip the copy.
    pub fn blit(&self, destination: &mut [u32], stride: usize, viewport: Rect) {
        if stride == 0 || self.width == 0 || self.height == 0 {
            return;
        }
        let height = destination.len() / stride;
        let x = i64::from(viewport.x) + self.offset_x as i64;
        let y = i64::from(viewport.y) + self.offset_y as i64;
        let left = x.max(0);
        let top = y.max(0);
        let right = (x + self.width as i64)
            .min(i64::from(viewport.x) + i64::from(viewport.width))
            .min(stride as i64);
        let bottom = (y + self.height as i64)
            .min(i64::from(viewport.y) + i64::from(viewport.height))
            .min(height as i64);
        if left >= right || top >= bottom {
            return;
        }
        let count = (right - left) as usize;
        for row in top..bottom {
            let source = (row - y) as usize * self.width + (left - x) as usize;
            let target = row as usize * stride + left as usize;
            destination[target..target + count]
                .copy_from_slice(&self.pixels[source..source + count]);
        }
    }
}
