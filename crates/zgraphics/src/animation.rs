//! Bounded, deterministic animation storage and clock advancement.
//! The host supplies monotonic milliseconds; this module never starts timers.
use alloc::{boxed::Box, sync::Arc, vec::Vec};

use crate::{Graphics, Image, StoredImage, protocol::Command};

pub(super) const MAX_FRAMES: usize = 512;

pub(crate) struct Frame {
    pub pixels: Arc<Vec<u8>>,
    gap: u32,
}

pub(crate) struct Animation {
    pub frames: Vec<Frame>,
    current: usize,
    state: u32,
    loops: u32,
    remaining: u32,
    deadline: Option<u64>,
    loading_wait: bool,
}

impl Animation {
    fn new(image: &Image) -> Self {
        Self {
            frames: alloc::vec![Frame {
                pixels: Arc::clone(&image.pixels),
                gap: 0
            }],
            current: 0,
            state: 1,
            loops: 0,
            remaining: 0,
            deadline: None,
            loading_wait: false,
        }
    }
}

fn frame(stored: &StoredImage, index: usize) -> Option<&Arc<Vec<u8>>> {
    match &stored.animation {
        Some(animation) => animation.frames.get(index).map(|frame| &frame.pixels),
        None if index == 0 => Some(&stored.image.pixels),
        None => None,
    }
}

fn canvas(size: usize, pixels: Option<&[u8]>, color: u32) -> Result<Vec<u8>, &'static str> {
    let mut out = Vec::new();
    out.try_reserve_exact(size)
        .map_err(|_| "ENOMEM:cannot allocate animation frame")?;
    if let Some(pixels) = pixels {
        out.extend_from_slice(pixels);
    } else {
        let rgba = color.to_be_bytes();
        for _ in 0..size / 4 {
            out.extend_from_slice(&rgba);
        }
    }
    Ok(out)
}

fn blend(dst: &mut [u8], src: &[u8], replace: bool) {
    if replace || src[3] == 255 {
        dst.copy_from_slice(src);
    } else if src[3] != 0 {
        let sa = u32::from(src[3]);
        let da = u32::from(dst[3]);
        let alpha = sa * 255 + da * (255 - sa);
        for channel in 0..3 {
            let numerator =
                u32::from(src[channel]) * sa * 255 + u32::from(dst[channel]) * da * (255 - sa);
            dst[channel] = ((numerator + alpha / 2) / alpha) as u8;
        }
        dst[3] = ((alpha + 127) / 255) as u8;
    }
}

impl Graphics {
    pub(crate) fn total_frames(&self) -> usize {
        self.images
            .iter()
            .map(|image| image.animation.as_ref().map_or(1, |a| a.frames.len()))
            .sum()
    }
    pub fn frame_count(&self, id: u32) -> usize {
        self.images
            .iter()
            .find(|image| image.id == id)
            .map_or(0, |image| {
                image
                    .animation
                    .as_ref()
                    .map_or(1, |animation| animation.frames.len())
            })
    }

    pub fn has_running_animations(&self) -> bool {
        self.images
            .iter()
            .any(|image| image.animation.as_ref().is_some_and(|a| a.state > 1))
    }

    pub fn animation_deadline(&self) -> Option<u64> {
        self.images
            .iter()
            .filter_map(|image| image.animation.as_ref())
            .filter(|animation| animation.state > 1)
            .filter_map(|animation| animation.deadline)
            .min()
    }

    pub(crate) fn store_frame(
        &mut self,
        id: u32,
        c: &Command,
        patch: Image,
    ) -> Result<(), &'static str> {
        let stored = self
            .images
            .iter()
            .find(|image| image.id == id)
            .ok_or("ENOENT:image not found")?;
        if stored.sixel_pixels.is_some() || c.x_offset > 1 {
            return Err("EINVAL:invalid animation composition");
        }
        let width = stored.image.width;
        let height = stored.image.height;
        if c.x
            .checked_add(patch.width)
            .is_none_or(|right| right > width)
            || c.y
                .checked_add(patch.height)
                .is_none_or(|bottom| bottom > height)
        {
            return Err("EINVAL:animation rectangle is outside image");
        }
        let count = self.frame_count(id);
        let editing = c.rows.checked_sub(1).map(|index| index as usize);
        if editing.is_some_and(|index| index >= count) {
            return Err("ENOENT:animation frame not found");
        }
        let base = editing.or_else(|| c.columns.checked_sub(1).map(|index| index as usize));
        if base.is_some_and(|index| frame(stored, index).is_none()) {
            return Err("ENOENT:animation base frame not found");
        }
        let total_frames = self.total_frames();
        if editing.is_none() && total_frames >= MAX_FRAMES {
            return Err("ENOSPC:animation frame count limit reached");
        }
        let size = width as usize * height as usize * 4;
        let replaced = editing
            .and_then(|index| frame(stored, index))
            .map_or(0, |p| p.capacity());
        let wanted = stored
            .bytes()
            .checked_sub(replaced)
            .and_then(|n| n.checked_add(size))
            .ok_or("ENOSPC:animation storage overflow")?;
        // Count every retained frame against the same per-image/scene quota.
        self.reserve_image(id, wanted)?;
        let stored = self.images.iter().find(|image| image.id == id).unwrap();
        let mut pixels = canvas(
            size,
            base.and_then(|index| frame(stored, index))
                .map(|p| p.as_slice()),
            c.y_offset,
        )?;
        for y in 0..patch.height as usize {
            for x in 0..patch.width as usize {
                let to = ((c.y as usize + y) * width as usize + c.x as usize + x) * 4;
                let from = (y * patch.width as usize + x) * 4;
                blend(
                    &mut pixels[to..to + 4],
                    &patch.pixels[from..from + 4],
                    c.x_offset == 1,
                );
            }
        }
        self.changed();
        let stored = self.images.iter_mut().find(|image| image.id == id).unwrap();
        let animation = stored
            .animation
            .get_or_insert_with(|| Box::new(Animation::new(&stored.image)));
        let gap = if c.z == 0 { 40 } else { c.z.max(0) as u32 };
        if let Some(index) = editing {
            let gap = if c.z == 0 {
                animation.frames[index].gap
            } else {
                gap
            };
            animation.frames[index] = Frame {
                pixels: Arc::new(pixels),
                gap,
            };
        } else {
            animation.frames.push(Frame {
                pixels: Arc::new(pixels),
                gap,
            });
            if animation.loading_wait {
                animation.current += 1;
                animation.deadline = None;
                animation.loading_wait = false;
            }
        }
        stored.image.pixels = Arc::clone(&animation.frames[animation.current].pixels);
        stored.image.generation = self.revision;
        self.used = self.images.iter().map(StoredImage::bytes).sum();
        Ok(())
    }

    pub(crate) fn control_animation(&mut self, id: u32, c: &Command) -> Result<(), &'static str> {
        let count = self.frame_count(id);
        if count == 0 {
            return Err("ENOENT:image not found");
        }
        if c.width > 3 {
            return Err("EINVAL:invalid animation state");
        }
        if c.columns as usize > count || c.rows as usize > count {
            return Err("ENOENT:animation frame not found");
        }
        let stored = self.images.iter_mut().find(|image| image.id == id).unwrap();
        if stored.sixel_pixels.is_some() {
            return Err("EINVAL:Sixel is not a Kitty animation");
        }
        let animation = stored
            .animation
            .get_or_insert_with(|| Box::new(Animation::new(&stored.image)));
        if c.z != 0 {
            let index = c
                .rows
                .checked_sub(1)
                .map_or(animation.current, |n| n as usize);
            animation.frames[index].gap = c.z.max(0) as u32;
            animation.deadline = None;
        }
        if c.height != 0 {
            animation.loops = c.height - 1;
            animation.remaining = animation.loops;
        }
        if c.width != 0 {
            animation.state = c.width;
            animation.remaining = animation.loops;
            animation.deadline = None;
            animation.loading_wait = false;
        }
        if c.columns != 0 {
            animation.current = c.columns as usize - 1;
            animation.deadline = None;
            animation.loading_wait = false;
            stored.image.pixels = Arc::clone(&animation.frames[animation.current].pixels);
            stored.image.generation = self.revision.wrapping_add(1);
        }
        self.changed();
        Ok(())
    }

    pub(crate) fn compose_frame(&mut self, id: u32, c: &Command) -> Result<(), &'static str> {
        let stored = self
            .images
            .iter()
            .find(|image| image.id == id)
            .ok_or("ENOENT:image not found")?;
        let source = c
            .rows
            .checked_sub(1)
            .ok_or("ENOENT:source frame not found")? as usize;
        let destination = c
            .columns
            .checked_sub(1)
            .ok_or("ENOENT:destination frame not found")? as usize;
        let source_pixels = frame(stored, source).ok_or("ENOENT:source frame not found")?;
        let destination_pixels =
            frame(stored, destination).ok_or("ENOENT:destination frame not found")?;
        let width = if c.width_crop == 0 {
            stored.image.width
        } else {
            c.width_crop
        };
        let height = if c.height_crop == 0 {
            stored.image.height
        } else {
            c.height_crop
        };
        let fits = |x: u32, y: u32| {
            x.checked_add(width)
                .is_some_and(|right| right <= stored.image.width)
                && y.checked_add(height)
                    .is_some_and(|bottom| bottom <= stored.image.height)
        };
        if !fits(c.x, c.y)
            || !fits(c.x_offset, c.y_offset)
            || source == destination
                && c.x < c.x_offset + width
                && c.x_offset < c.x + width
                && c.y < c.y_offset + height
                && c.y_offset < c.y + height
        {
            return Err("EINVAL:invalid or overlapping frame rectangles");
        }
        let stride = stored.image.width as usize;
        let mut pixels = canvas(destination_pixels.len(), Some(destination_pixels), 0)?;
        for y in 0..height as usize {
            for x in 0..width as usize {
                let from = ((c.y_offset as usize + y) * stride + c.x_offset as usize + x) * 4;
                let to = ((c.y as usize + y) * stride + c.x as usize + x) * 4;
                blend(
                    &mut pixels[to..to + 4],
                    &source_pixels[from..from + 4],
                    c.no_cursor_move,
                );
            }
        }
        // Existing fully rendered frames have exactly this size; composition
        // replaces one frame and cannot grow retained storage.
        self.changed();
        let stored = self.images.iter_mut().find(|image| image.id == id).unwrap();
        let animation = stored
            .animation
            .get_or_insert_with(|| Box::new(Animation::new(&stored.image)));
        animation.frames[destination].pixels = Arc::new(pixels);
        stored.image.pixels = Arc::clone(&animation.frames[animation.current].pixels);
        stored.image.generation = self.revision;
        self.used = self.images.iter().map(StoredImage::bytes).sum();
        Ok(())
    }

    pub(crate) fn delete_frame(&mut self, id: u32, c: &Command) -> Result<(), &'static str> {
        let count = self.frame_count(id);
        if count == 0 {
            return Err("ENOENT:image not found");
        }
        let index = c.rows.checked_sub(1).map(|n| n as usize);
        if index.is_some_and(|n| n >= count) {
            return Err("ENOENT:animation frame not found");
        }
        if count == 1 {
            if c.delete.is_ascii_uppercase() {
                self.placements.retain(|p| p.image_id != id);
                self.images.retain(|image| image.id != id);
                self.used = self.images.iter().map(StoredImage::bytes).sum();
                self.prune_dependencies();
                self.changed();
            }
            return Ok(());
        }
        self.changed();
        let stored = self.images.iter_mut().find(|image| image.id == id).unwrap();
        let animation = stored.animation.as_mut().unwrap();
        let index = index.unwrap_or(animation.current);
        animation.frames.remove(index);
        if animation.current > index {
            animation.current -= 1;
        }
        animation.current = animation.current.min(animation.frames.len() - 1);
        animation.deadline = None;
        animation.loading_wait = false;
        stored.image.pixels = Arc::clone(&animation.frames[animation.current].pixels);
        stored.image.generation = self.revision;
        self.used = self.images.iter().map(StoredImage::bytes).sum();
        Ok(())
    }

    /// Advance visible animations and return the next monotonic wake deadline.
    /// Hidden/closed views pass an empty list, which suspends clocks. A tick
    /// skips at most one complete frame set per image, even after a long stall.
    /// Gapless-only animations and loading animations at EOF schedule no wake.
    pub fn advance_animations(&mut self, now: u64, visible: &[u32]) -> Option<u64> {
        let mut next: Option<u64> = None;
        let mut changed = false;
        let generation = self.revision.wrapping_add(1);
        for stored in &mut self.images {
            let Some(animation) = &mut stored.animation else {
                continue;
            };
            if animation.state == 1 || !visible.contains(&stored.id) {
                animation.deadline = None;
                continue;
            }
            if animation.frames.iter().all(|frame| frame.gap == 0) {
                animation.deadline = None;
                continue;
            }
            let previous = animation.current;
            let mut deadline = animation.deadline.unwrap_or_else(|| {
                now.saturating_add(u64::from(animation.frames[animation.current].gap))
            });
            for _ in 0..animation.frames.len() {
                if deadline > now {
                    break;
                }
                let last = animation.current + 1 == animation.frames.len();
                if last && animation.state == 2 {
                    animation.loading_wait = true;
                    animation.deadline = None;
                    break;
                }
                if last && animation.loops != 0 {
                    if animation.remaining <= 1 {
                        animation.state = 1;
                        animation.remaining = animation.loops;
                        animation.deadline = None;
                        break;
                    }
                    animation.remaining -= 1;
                }
                animation.current = (animation.current + 1) % animation.frames.len();
                deadline =
                    deadline.saturating_add(u64::from(animation.frames[animation.current].gap));
            }
            if previous != animation.current {
                stored.image.pixels = Arc::clone(&animation.frames[animation.current].pixels);
                stored.image.generation = generation;
                changed = true;
            }
            if animation.state > 1 && !animation.loading_wait {
                // Extremely late clocks must not create a permanently expired
                // deadline and a busy loop. Resume from the displayed frame.
                if deadline <= now {
                    let Some(future) =
                        now.checked_add(u64::from(animation.frames[animation.current].gap.max(1)))
                    else {
                        animation.deadline = None;
                        continue;
                    };
                    deadline = future;
                }
                animation.deadline = Some(deadline);
                next = Some(next.map_or(deadline, |old| old.min(deadline)));
            }
        }
        if changed {
            self.changed();
        }
        next
    }
}
