//! Bounded XRender uploads and terminal image placement.
use super::*;

impl Window {
    pub fn graphics_changed(&self, terminal: &Terminal) -> bool {
        self.graphics_revision != terminal.graphics().revision()
            || self.graphics_alternate != terminal.modes().alternate_screen
    }

    pub(super) fn discard_upload(&mut self, index: usize) {
        let upload = self.images.swap_remove(index);
        self.image_bytes = self.image_bytes.saturating_sub(upload.bytes);
        unsafe {
            XRenderFreePicture(self.display, upload.picture);
            XFreePixmap(self.display, upload.pixmap);
        }
    }

    pub(super) fn image_picture(
        &mut self,
        image_id: u32,
        image: &zterm::graphics::Image,
    ) -> Option<Xid> {
        const CACHE_LIMIT: usize = 64 * 1024 * 1024;
        self.image_clock = self.image_clock.wrapping_add(1);
        if let Some(upload) = self
            .images
            .iter_mut()
            .find(|entry| entry.image_id == image_id && entry.generation == image.generation)
        {
            upload.used = self.image_clock;
            return Some(upload.picture);
        }
        let bytes = (image.width as usize)
            .checked_mul(image.height as usize)?
            .checked_mul(4)?;
        if bytes == 0 || bytes > CACHE_LIMIT || bytes != image.pixels.len() {
            return None;
        }
        while self.image_bytes.saturating_add(bytes) > CACHE_LIMIT || self.images.len() >= 256 {
            let index = self
                .images
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.used)?
                .0;
            self.discard_upload(index);
        }
        unsafe {
            // XDestroyImage owns and frees this C allocation. Rust never hands
            // an alloc-owned buffer to Xlib's free callback.
            let data = malloc(bytes).cast::<u32>();
            if data.is_null() {
                return None;
            }
            for (index, pixel) in image.pixels.as_chunks::<4>().0.iter().enumerate() {
                let alpha = u32::from(pixel[3]);
                let premultiply = |channel: u8| (u32::from(channel) * alpha + 127) / 255;
                data.add(index).write(
                    (alpha << 24)
                        | (premultiply(pixel[0]) << 16)
                        | (premultiply(pixel[1]) << 8)
                        | premultiply(pixel[2]),
                );
            }
            let ximage = XCreateImage(
                self.display,
                self.visual,
                32,
                2,
                0,
                data.cast(),
                image.width,
                image.height,
                32,
                (image.width * 4) as i32,
            );
            if ximage.is_null() {
                free(data.cast());
                return None;
            }
            let format = XRenderFindStandardFormat(self.display, 0); // PictStandardARGB32
            if format.is_null() {
                XDestroyImage(ximage);
                return None;
            }
            let pixmap = XCreatePixmap(self.display, self.id, image.width, image.height, 32);
            let gc = XCreateGC(self.display, pixmap, 0, core::ptr::null_mut());
            if pixmap == 0 || gc.is_null() {
                if !gc.is_null() {
                    XFreeGC(self.display, gc);
                }
                if pixmap != 0 {
                    XFreePixmap(self.display, pixmap);
                }
                XDestroyImage(ximage);
                return None;
            }
            XPutImage(
                self.display,
                pixmap,
                gc,
                ximage,
                0,
                0,
                0,
                0,
                image.width,
                image.height,
            );
            XFreeGC(self.display, gc);
            XDestroyImage(ximage);
            let picture = XRenderCreatePicture(self.display, pixmap, format, 0, core::ptr::null());
            if picture == 0 {
                XFreePixmap(self.display, pixmap);
                return None;
            }
            XRenderSetPictureFilter(
                self.display,
                picture,
                c"nearest".as_ptr(),
                core::ptr::null(),
                0,
            );
            self.images.push(ImageUpload {
                image_id,
                generation: image.generation,
                picture,
                pixmap,
                bytes,
                used: self.image_clock,
            });
            self.image_bytes += bytes;
            Some(picture)
        }
    }

    /// Composite only the damaged row. The transform maps destination pixels
    /// back into the requested source crop, without allocating a resized image.
    pub(super) fn draw_images(
        &mut self,
        terminal: &Terminal,
        offset: usize,
        row: usize,
        layer: u8,
    ) {
        let width = terminal.cols() as i64 * i64::from(self.cw);
        let row_top = row as i64 * i64::from(self.ch);
        for placement in terminal.graphics_draws(offset) {
            let wanted_layer = if placement.z < -1_073_741_824 {
                0
            } else if placement.z < 0 {
                1
            } else {
                2
            };
            if layer != wanted_layer || placement.dest.width == 0 || placement.dest.height == 0 {
                continue;
            }
            let dx = i64::from(placement.dest.x);
            let dy = i64::from(placement.dest.y);
            let clip = placement.clip;
            let left = dx.max(0).max(clip.map_or(i64::MIN, |c| i64::from(c.x)));
            let top = dy
                .max(row_top)
                .max(clip.map_or(i64::MIN, |c| i64::from(c.y)));
            let right = (dx + i64::from(placement.dest.width))
                .min(width)
                .min(clip.map_or(i64::MAX, |c| i64::from(c.x) + i64::from(c.width)));
            let bottom = (dy + i64::from(placement.dest.height))
                .min(row_top + i64::from(self.ch))
                .min(clip.map_or(i64::MAX, |c| i64::from(c.y) + i64::from(c.height)));
            if left >= right || top >= bottom {
                continue;
            }
            let Some(picture) = self.image_picture(placement.image_id, placement.image) else {
                continue;
            };
            let fixed = |source: u32, destination: u32| {
                ((u64::from(source) << 16) / u64::from(destination)).min(i32::MAX as u64) as i32
            };
            let transform = Transform {
                matrix: [
                    [
                        fixed(placement.source.width, placement.dest.width),
                        0,
                        (placement.source.x as i32) << 16,
                    ],
                    [
                        0,
                        fixed(placement.source.height, placement.dest.height),
                        (placement.source.y as i32) << 16,
                    ],
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
                    (left - dx) as i32,
                    (top - dy) as i32,
                    0,
                    0,
                    left as i32 + self.padding,
                    top as i32 + self.padding,
                    (right - left) as u32,
                    (bottom - top) as u32,
                );
            }
        }
    }
}
