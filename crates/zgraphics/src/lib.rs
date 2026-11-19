#![doc = include_str!("../README.md")]
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::sync::Arc;
use alloc::vec::Vec;

mod decode;
mod protocol;
mod sixel;
mod stream;

pub use sixel::Palette as SixelPalette;
pub use stream::CommandStream;

use protocol::Command;

/// Limits apply independently to each graphics scene (usually one screen).
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_dimension: u32,
    pub max_image_bytes: usize,
    pub max_total_bytes: usize,
    pub max_transfer_bytes: usize,
    pub max_images: usize,
    pub max_placements: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_dimension: 8192,
            max_image_bytes: 16 * 1024 * 1024,
            max_total_bytes: 32 * 1024 * 1024,
            max_transfer_bytes: 16 * 1024 * 1024,
            max_images: 128,
            max_placements: 512,
        }
    }
}

/// Canonical sRGB, straight-alpha RGBA pixels. Row stride is `width * 4`.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Arc<Vec<u8>>,
    /// Changes whenever image contents are replaced, including ID reuse.
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// A renderer draws `source` scaled into `dest`, clipped to the text viewport.
/// Draws are sorted by z-index, then image ID. `z < 0` is behind text;
/// `z < -1073741824` is also behind explicitly colored cell backgrounds.
pub struct Draw<'a> {
    pub image: &'a Image,
    pub image_id: u32,
    pub placement_id: u32,
    pub source: Rect,
    pub dest: PixelRect,
    pub z: i32,
}

/// Grid context at the instant the final image chunk is received.
#[derive(Clone, Copy, Debug)]
pub struct Context {
    pub col: usize,
    pub row: usize,
    pub cols: usize,
    pub rows: usize,
    pub cell_width: u32,
    pub cell_height: u32,
}

/// A successful placement can request a cursor move after its rectangle.
#[derive(Clone, Copy, Debug, Default)]
pub struct Outcome {
    pub cursor_move: Option<(usize, usize)>,
}

/// Successfully stored sixel geometry and the resulting color registers.
pub struct SixelOutcome {
    pub width: u32,
    pub height: u32,
    pub palette: SixelPalette,
}

struct StoredImage {
    id: u32,
    number: u32,
    anonymous: bool,
    /// Remaining nontransparent source pixels; None identifies Kitty storage.
    sixel_pixels: Option<usize>,
    image: Image,
}

struct Placement {
    image_id: u32,
    id: u32,
    col: i32,
    row: i32,
    source: Rect,
    columns: u32,
    rows: u32,
    x_offset: u32,
    y_offset: u32,
    z: i32,
    pixel_scale_y: u32,
}

impl Placement {
    fn geometry(&self, cell_width: u32, cell_height: u32) -> (u32, u32, u32, u32) {
        let cw = cell_width.max(1);
        let ch = cell_height.max(1);
        let sw = u64::from(self.source.width);
        let sh = u64::from(self.source.height) * u64::from(self.pixel_scale_y);
        let target_w = u64::from(self.columns) * u64::from(cw);
        let target_h = u64::from(self.rows) * u64::from(ch);
        let (width, height) = match (self.columns, self.rows) {
            (0, 0) => (sw, sh),
            (_, 0) => (target_w, (sh * target_w / sw).max(1)),
            (0, _) => ((sw * target_h / sh).max(1), target_h),
            _ if sw * target_h > sh * target_w => (target_w, (sh * target_w / sw).max(1)),
            _ => ((sw * target_h / sh).max(1), target_h),
        };
        let box_w = if self.columns == 0 { width } else { target_w };
        let box_h = if self.rows == 0 { height } else { target_h };
        (
            width.min(u32::MAX as u64) as u32,
            height.min(u32::MAX as u64) as u32,
            box_w.saturating_sub(width).div_ceil(2) as u32,
            box_h.saturating_sub(height).div_ceil(2) as u32,
        )
    }

    fn rect(&self, cw: u32, ch: u32, history: usize) -> PixelRect {
        let (width, height, inset_x, inset_y) = self.geometry(cw, ch);
        PixelRect {
            x: clamp_i32(i64::from(self.col) * i64::from(cw) + i64::from(self.x_offset + inset_x)),
            y: clamp_i32(
                (i64::from(self.row) + history as i64) * i64::from(ch)
                    + i64::from(self.y_offset + inset_y),
            ),
            width,
            height,
        }
    }
}

fn clamp_i32(n: i64) -> i32 {
    n.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

struct Transfer {
    command: Command,
    bytes: Vec<u8>,
}

/// A bounded image store and Kitty static-image command processor.
/// No platform calls, filesystem access, threads, or rendering backend.
pub struct Graphics {
    limits: Limits,
    images: Vec<StoredImage>,
    placements: Vec<Placement>,
    pending: Option<Transfer>,
    sixel: Option<Box<sixel::Decoder>>,
    used: usize,
    revision: u64,
    next_id: u32,
    cell_width: u32,
    cell_height: u32,
    visible_rows: usize,
}

impl Default for Graphics {
    fn default() -> Self {
        Self::new(Limits::default())
    }
}

impl Graphics {
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            images: Vec::new(),
            placements: Vec::new(),
            pending: None,
            sixel: None,
            used: 0,
            revision: 0,
            next_id: 1,
            cell_width: 8,
            cell_height: 16,
            visible_rows: 24,
        }
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }
    pub const fn image_bytes(&self) -> usize {
        self.used
    }
    pub fn pending_bytes(&self) -> usize {
        self.pending.as_ref().map_or(0, |t| t.bytes.len())
            + self
                .sixel
                .as_ref()
                .map_or(0, |decoder| decoder.allocated_bytes())
    }
    pub fn image_count(&self) -> usize {
        self.images.len()
    }
    pub fn placement_count(&self) -> usize {
        self.placements.len()
    }
    pub fn is_empty(&self) -> bool {
        self.placements.is_empty()
    }
    /// Erase Sixel pixels intersecting a rectangle in the live cell grid.
    ///
    /// Kitty images and history outside this rectangle are unchanged. Erasure
    /// uses source-pixel granularity: a scaled pixel touching the rectangle is
    /// cleared completely. At a nonaligned vertical boundary this can clear up
    /// to pixel_scale_y - 1 additional display pixels. Unscaled images retain
    /// every pixel outside the requested rectangle exactly.
    pub fn erase_sixel_cells(&mut self, col: usize, row: usize, cols: usize, rows: usize) {
        if self.placements.is_empty() || cols == 0 || rows == 0 {
            return;
        }
        let left = col.min(65535) as i64 * i64::from(self.cell_width);
        let top = row.min(65535) as i64 * i64::from(self.cell_height);
        let right = col.saturating_add(cols).min(65535) as i64 * i64::from(self.cell_width);
        let bottom = row.saturating_add(rows).min(65535) as i64 * i64::from(self.cell_height);
        let generation = self.revision.wrapping_add(1);
        let mut changed = false;
        for stored in &mut self.images {
            let Some(remaining) = &mut stored.sixel_pixels else {
                continue;
            };
            for placement in self.placements.iter().filter(|p| p.image_id == stored.id) {
                let rect = placement.rect(self.cell_width, self.cell_height, 0);
                let x = i64::from(rect.x);
                let y = i64::from(rect.y);
                let x0 = left.max(x);
                let y0 = top.max(y);
                let x1 = right.min(x + i64::from(rect.width));
                let y1 = bottom.min(y + i64::from(rect.height));
                if x0 >= x1 || y0 >= y1 {
                    continue;
                }
                let source = placement.source;
                let sx0 = source.x as usize
                    + ((x0 - x) as u64 * u64::from(source.width) / u64::from(rect.width)) as usize;
                let sx1 = source.x as usize
                    + ((x1 - x) as u64 * u64::from(source.width)).div_ceil(u64::from(rect.width))
                        as usize;
                let sy0 = source.y as usize
                    + ((y0 - y) as u64 * u64::from(source.height) / u64::from(rect.height))
                        as usize;
                let sy1 = source.y as usize
                    + ((y1 - y) as u64 * u64::from(source.height)).div_ceil(u64::from(rect.height))
                        as usize;
                let width = stored.image.width as usize;
                let pixels = Arc::make_mut(&mut stored.image.pixels);
                for source_row in sy0..sy1 {
                    let begin = (source_row * width + sx0) * 4;
                    let end = (source_row * width + sx1) * 4;
                    for pixel in pixels[begin..end].as_chunks_mut::<4>().0 {
                        if pixel[3] != 0 {
                            pixel.fill(0);
                            *remaining -= 1;
                            stored.image.generation = generation;
                            changed = true;
                        }
                    }
                }
                // Also reclaim a transparent-only Sixel when its area is erased.
                changed |= *remaining == 0;
            }
        }
        if changed {
            let images = &self.images;
            self.placements.retain(|placement| {
                images
                    .iter()
                    .any(|image| image.id == placement.image_id && image.sixel_pixels != Some(0))
            });
            let used = &mut self.used;
            self.images.retain(|image| {
                if image.sixel_pixels == Some(0) {
                    *used -= image.image.pixels.capacity();
                    false
                } else {
                    true
                }
            });
            self.changed();
        }
    }

    pub fn image(&self, id: u32) -> Option<&Image> {
        self.images
            .iter()
            .find(|image| image.id == id)
            .map(|image| &image.image)
    }

    fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// Force renderer cache invalidation, for example after swapping screens.
    pub fn invalidate(&mut self) {
        self.changed();
    }

    pub fn set_viewport(&mut self, cell_width: u32, cell_height: u32, rows: usize) {
        let cw = cell_width.clamp(1, 4096);
        let ch = cell_height.clamp(1, 4096);
        let rows = rows.clamp(1, 65535);
        if (self.cell_width, self.cell_height, self.visible_rows) != (cw, ch, rows) {
            self.cell_width = cw;
            self.cell_height = ch;
            self.visible_rows = rows;
            self.changed();
        }
    }

    pub fn draws(&self, history_offset: usize) -> impl Iterator<Item = Draw<'_>> {
        let history_offset = history_offset.min(i32::MAX as usize);
        self.placements.iter().filter_map(move |p| {
            let image = self.image(p.image_id)?;
            let dest = p.rect(self.cell_width, self.cell_height, history_offset);
            if i64::from(dest.y) + i64::from(dest.height) <= 0
                || i64::from(dest.y) >= (self.visible_rows as i64) * i64::from(self.cell_height)
            {
                return None;
            }
            Some(Draw {
                image,
                image_id: p.image_id,
                placement_id: p.id,
                source: p.source,
                dest,
                z: p.z,
            })
        })
    }

    /// Cancel only a partial transmission. Completed images remain untouched.
    pub fn abort_transfer(&mut self) {
        self.pending = None;
    }

    /// Begin a sixel body after a validated DCS `q` header. Color registers and
    /// decoding storage are allocated only while a sixel stream is active.
    pub fn begin_sixel(
        &mut self,
        aspect: u16,
        transparent: bool,
        background: [u8; 3],
        palette: Option<&SixelPalette>,
    ) {
        self.abort_transfer();
        self.sixel = Some(Box::new(sixel::Decoder::new(
            self.limits,
            aspect,
            transparent,
            background,
            palette,
        )));
    }

    /// Feed one body byte. A malformed or excessive stream drops its partial
    /// allocation immediately; later bytes remain ignored until the next DCS.
    pub fn sixel_put(&mut self, byte: u8) {
        if self
            .sixel
            .as_mut()
            .is_some_and(|decoder| decoder.push(byte).is_err())
        {
            self.sixel = None;
        }
    }

    pub fn abort_sixel(&mut self) {
        self.sixel = None;
    }

    /// Commit only after an actual ST. No reply is sent by the sixel protocol.
    /// The terminal, rather than the scene, implements scrolling/cursor policy.
    pub fn finish_sixel(&mut self, context: Context) -> Option<SixelOutcome> {
        let decoded = self.sixel.take()?.finish().ok()?;
        if decoded.width == 0 || decoded.height == 0 {
            return Some(SixelOutcome {
                width: 0,
                height: 0,
                palette: decoded.palette,
            });
        }
        if self.placements.len() >= self.limits.max_placements {
            return None;
        }
        let id = self.allocate_id();
        self.reserve_image(id, decoded.pixels.capacity()).ok()?;
        self.set_viewport(context.cell_width, context.cell_height, context.rows);
        self.changed();
        self.used += decoded.pixels.capacity();
        let result = SixelOutcome {
            width: decoded.width,
            height: decoded.height * decoded.scale_y,
            palette: decoded.palette,
        };
        self.images.push(StoredImage {
            id,
            number: 0,
            anonymous: true,
            sixel_pixels: Some(
                decoded
                    .pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|pixel| pixel[3] != 0)
                    .count(),
            ),
            image: Image {
                width: decoded.width,
                height: decoded.height,
                pixels: Arc::new(decoded.pixels),
                generation: self.revision,
            },
        });
        self.placements.push(Placement {
            image_id: id,
            id: 0,
            col: context.col.min(context.cols.saturating_sub(1)).min(65534) as i32,
            row: context.row.min(context.rows.saturating_sub(1)).min(65534) as i32,
            source: Rect {
                x: 0,
                y: 0,
                width: result.width,
                height: result.height / decoded.scale_y,
            },
            columns: 0,
            rows: 0,
            x_offset: 0,
            y_offset: 0,
            z: -1,
            pixel_scale_y: decoded.scale_y,
        });
        self.placements.sort_by_key(|p| (p.z, p.image_id));
        Some(result)
    }

    /// Clear all stored images, placements and partial transfers.
    pub fn clear(&mut self) {
        self.images.clear();
        self.placements.clear();
        self.pending = None;
        self.sixel = None;
        self.used = 0;
        self.changed();
    }

    /// Follow a text-region scroll. A positive delta moves content down.
    /// Only a full-screen upward scroll may retain placements in scrollback.
    pub fn scroll(
        &mut self,
        top: usize,
        bottom: usize,
        delta: i32,
        history: usize,
        save_history: bool,
    ) {
        if delta == 0 || self.placements.is_empty() {
            return;
        }
        let history = history.min(i32::MAX as usize);
        let full = top == 0 && bottom.saturating_add(1) == self.visible_rows;
        let cw = self.cell_width;
        let ch = self.cell_height;
        self.placements.retain_mut(|p| {
            if full && save_history && delta < 0 {
                p.row = p.row.saturating_add(delta);
                let rect = p.rect(cw, ch, 0);
                i64::from(rect.y) + i64::from(rect.height) > -(history as i64) * i64::from(ch)
            } else if p.row >= top as i32 && p.row <= bottom as i32 {
                p.row = p.row.saturating_add(delta);
                p.row >= top as i32 && p.row <= bottom as i32
            } else {
                true
            }
        });
        self.collect_unreferenced_anonymous();
        self.changed();
    }

    /// Drop placements whose last pixel is older than retained text history.
    pub fn trim_history(&mut self, history: usize) {
        let history = history.min(i32::MAX as usize);
        let cw = self.cell_width;
        let ch = self.cell_height;
        let before = self.placements.len();
        self.placements.retain(|p| {
            let r = p.rect(cw, ch, 0);
            i64::from(r.y) + i64::from(r.height) > -(history as i64) * i64::from(ch)
        });
        if before != self.placements.len() {
            self.changed();
        }
        self.collect_unreferenced_anonymous();
    }

    fn collect_unreferenced_anonymous(&mut self) {
        // Identified images may intentionally be transmitted before placement.
        // Keep them until explicit deletion or quota eviction.
        let placements = &self.placements;
        let mut used = 0;
        self.images.retain(|image| {
            let keep = !image.anonymous || placements.iter().any(|p| p.image_id == image.id);
            if keep {
                used += image.image.pixels.capacity();
            }
            keep
        });
        self.used = used;
    }

    fn allocate_id(&mut self) -> u32 {
        loop {
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1).max(1);
            if id != 0 && !self.images.iter().any(|image| image.id == id) {
                return id;
            }
        }
    }

    fn resolve(&self, c: &Command) -> Option<u32> {
        if c.id != 0 {
            return Some(c.id);
        }
        if c.number != 0 {
            return self
                .images
                .iter()
                .rev()
                .find(|i| i.number == c.number)
                .map(|i| i.id);
        }
        None
    }

    fn reserve_image(&mut self, id: u32, size: usize) -> Result<(), &'static str> {
        if size > self.limits.max_image_bytes || size > self.limits.max_total_bytes {
            return Err("ENOSPC:image exceeds memory limit");
        }
        loop {
            let replaced = self
                .images
                .iter()
                .find(|i| i.id == id)
                .map_or(0, |i| i.image.pixels.capacity());
            let count = self.images.len() + usize::from(replaced == 0);
            if self.used - replaced + size <= self.limits.max_total_bytes
                && count <= self.limits.max_images
            {
                return Ok(());
            }
            let Some(index) = self.images.iter().position(|image| {
                image.id != id && !self.placements.iter().any(|p| p.image_id == image.id)
            }) else {
                return Err("ENOSPC:image storage is full");
            };
            self.used -= self.images.remove(index).image.pixels.capacity();
            self.changed();
        }
    }

    /// Process one bounded APC body, including its leading `G` but not ESC/ST.
    /// Replies are appended atomically up to 64 KiB. A malformed command cannot
    /// modify an already decoded image. Fragment buffering is bounded separately.
    pub fn command(&mut self, body: &[u8], context: Context, replies: &mut Vec<u8>) -> Outcome {
        let (command, payload) = match protocol::parse(body) {
            Ok(v) => v,
            Err(_) => {
                self.abort_transfer();
                return Outcome::default();
            }
        };
        self.process(command, payload, context, replies)
    }

    fn reject_transfer(&mut self, error: &'static str, replies: &mut Vec<u8>) {
        if let Some(transfer) = self.pending.take() {
            self.respond(&transfer.command, transfer.command.id, error, replies);
        }
    }

    fn process(
        &mut self,
        command: Command,
        payload: &[u8],
        context: Context,
        replies: &mut Vec<u8>,
    ) -> Outcome {
        self.set_viewport(context.cell_width, context.cell_height, context.rows);
        let context = Context {
            col: context.col.min(context.cols.saturating_sub(1)).min(65534),
            row: context.row.min(context.rows.saturating_sub(1)).min(65534),
            cols: context.cols.clamp(1, 65535),
            rows: self.visible_rows,
            cell_width: self.cell_width,
            cell_height: self.cell_height,
        };
        if command.action == b'q' && command.feature != 0 {
            self.feature_query(&command, replies);
            return Outcome::default();
        }
        if command.continuation
            && self.pending.as_ref().is_some_and(|pending| {
                !command.action_specified || pending.command.action == command.action
            })
        {
            let mut transfer = self.pending.take().unwrap();
            if let Some(q) = command.quiet_override {
                transfer.command.quiet = q;
            }
            transfer.command.more = command.more;
            let result =
                decode::base64_append(payload, &mut transfer.bytes, self.limits.max_transfer_bytes);
            if let Err(error) = result {
                self.respond(&transfer.command, transfer.command.id, error, replies);
                return Outcome::default();
            }
            if transfer.command.more {
                self.pending = Some(transfer);
                return Outcome::default();
            }
            return self.finish(transfer.command, transfer.bytes, context, replies);
        }
        self.abort_transfer();
        if command.id != 0 && command.number != 0 {
            self.respond(
                &command,
                command.id,
                "EINVAL:i and I are mutually exclusive",
                replies,
            );
            return Outcome::default();
        }
        if command.unsupported {
            self.respond(
                &command,
                command.id,
                "ENOTSUP:unsupported graphics option",
                replies,
            );
            return Outcome::default();
        }
        match command.action {
            b'd' => {
                let result = self.delete(&command, context);
                if let Err(error) = result {
                    self.respond(&command, command.id, error, replies);
                }
                Outcome::default()
            }
            b'p' => {
                let id = self.resolve(&command).unwrap_or(command.id);
                let result = self.place(id, &command, context);
                self.respond(
                    &command,
                    id,
                    result.as_ref().err().copied().unwrap_or("OK"),
                    replies,
                );
                result.unwrap_or_default()
            }
            b't' | b'T' | b'q' => {
                if command.transport != b'd' {
                    self.respond(
                        &command,
                        command.id,
                        "ENOTSUP:only direct transmission is supported",
                        replies,
                    );
                    return Outcome::default();
                }
                if !matches!(command.format, 24 | 32 | 100) {
                    self.respond(
                        &command,
                        command.id,
                        "ENOTSUP:unsupported pixel format",
                        replies,
                    );
                    return Outcome::default();
                }
                if command.format != 100
                    && decode::pixel_size(command.width, command.height, self.limits).is_err()
                {
                    self.respond(
                        &command,
                        command.id,
                        "E2BIG:image dimensions exceed limit",
                        replies,
                    );
                    return Outcome::default();
                }
                let mut bytes = Vec::new();
                if let Err(error) =
                    decode::base64_append(payload, &mut bytes, self.limits.max_transfer_bytes)
                {
                    self.respond(&command, command.id, error, replies);
                    return Outcome::default();
                }
                if command.more {
                    self.pending = Some(Transfer { command, bytes });
                    Outcome::default()
                } else {
                    self.finish(command, bytes, context, replies)
                }
            }
            _ => {
                self.respond(
                    &command,
                    command.id,
                    "ENOTSUP:unsupported graphics action",
                    replies,
                );
                Outcome::default()
            }
        }
    }

    fn finish(
        &mut self,
        command: Command,
        bytes: Vec<u8>,
        context: Context,
        replies: &mut Vec<u8>,
    ) -> Outcome {
        let image = decode::image(&command, bytes, self.limits);
        let mut image = match image {
            Ok(image) => image,
            Err(error) => {
                self.respond(&command, command.id, error, replies);
                return Outcome::default();
            }
        };
        if command.action == b'q' {
            self.respond(&command, command.id, "OK", replies);
            return Outcome::default();
        }
        let id = if command.id == 0 {
            self.allocate_id()
        } else {
            command.id
        };
        // Reject placement geometry before replacing a previously valid image.
        if command.action == b'T'
            && let Err(error) = self.placement(id, &image, &command, context)
        {
            self.respond(&command, id, error, replies);
            return Outcome::default();
        }
        if command.action == b'T'
            && self.placements.iter().filter(|p| p.image_id != id).count()
                >= self.limits.max_placements
        {
            self.respond(&command, id, "ENOSPC:placement limit reached", replies);
            return Outcome::default();
        }
        if let Err(error) = self.reserve_image(id, image.pixels.capacity()) {
            self.respond(&command, id, error, replies);
            return Outcome::default();
        }
        self.changed();
        image.generation = self.revision;
        if let Some(index) = self.images.iter().position(|i| i.id == id) {
            self.used -= self.images.remove(index).image.pixels.capacity();
            self.placements.retain(|p| p.image_id != id);
        }
        self.used += image.pixels.capacity();
        self.images.push(StoredImage {
            id,
            number: command.number,
            anonymous: command.id == 0 && command.number == 0,
            sixel_pixels: None,
            image,
        });
        let result = if command.action == b'T' {
            self.place(id, &command, context)
        } else {
            Ok(Outcome::default())
        };
        self.respond(
            &command,
            id,
            result.as_ref().err().copied().unwrap_or("OK"),
            replies,
        );
        result.unwrap_or_default()
    }

    fn placement(
        &self,
        id: u32,
        image: &Image,
        c: &Command,
        ctx: Context,
    ) -> Result<Placement, &'static str> {
        if c.columns > 65535
            || c.rows > 65535
            || c.x_offset >= ctx.cell_width.max(1)
            || c.y_offset >= ctx.cell_height.max(1)
        {
            return Err("EINVAL:invalid placement geometry");
        }
        if c.x >= image.width || c.y >= image.height {
            return Err("EINVAL:source rectangle is outside image");
        }
        let source = Rect {
            x: c.x,
            y: c.y,
            width: if c.width_crop == 0 {
                image.width - c.x
            } else {
                c.width_crop.min(image.width - c.x)
            },
            height: if c.height_crop == 0 {
                image.height - c.y
            } else {
                c.height_crop.min(image.height - c.y)
            },
        };
        Ok(Placement {
            image_id: id,
            id: c.placement,
            col: ctx.col as i32,
            row: ctx.row as i32,
            source,
            columns: c.columns,
            rows: c.rows,
            x_offset: c.x_offset,
            y_offset: c.y_offset,
            z: c.z,
            pixel_scale_y: 1,
        })
    }

    fn place(&mut self, id: u32, c: &Command, ctx: Context) -> Result<Outcome, &'static str> {
        let image = self.image(id).ok_or("ENOENT:image not found")?;
        let p = self.placement(id, image, c, ctx)?;
        let (w, h, _, _) = p.geometry(ctx.cell_width, ctx.cell_height);
        let cols = if c.columns != 0 {
            c.columns
        } else {
            (w + c.x_offset).div_ceil(ctx.cell_width.max(1))
        };
        let rows = if c.rows != 0 {
            c.rows
        } else {
            (h + c.y_offset).div_ceil(ctx.cell_height.max(1))
        };
        let replacement = (c.placement != 0)
            .then(|| {
                self.placements
                    .iter()
                    .position(|p| p.image_id == id && p.id == c.placement)
            })
            .flatten();
        if replacement.is_none() && self.placements.len() >= self.limits.max_placements {
            return Err("ENOSPC:placement limit reached");
        }
        if let Some(index) = replacement {
            self.placements.remove(index);
        }
        self.placements.push(p);
        self.placements.sort_by_key(|p| (p.z, p.image_id));
        self.changed();
        Ok(Outcome {
            cursor_move: if c.no_cursor_move {
                None
            } else {
                Some((cols as usize, rows as usize))
            },
        })
    }

    fn delete(&mut self, c: &Command, ctx: Context) -> Result<(), &'static str> {
        let selector = c.delete.to_ascii_lowercase();
        if !matches!(
            selector,
            b'a' | b'i' | b'n' | b'c' | b'p' | b'q' | b'r' | b'x' | b'y' | b'z'
        ) {
            return Err("ENOTSUP:unsupported deletion selector");
        }
        let id = self.resolve(c).unwrap_or(c.id);
        let cw = self.cell_width;
        let ch = self.cell_height;
        let cx = if selector == b'c' {
            ctx.col as i64
        } else {
            i64::from(c.x.saturating_sub(1))
        } * i64::from(cw);
        let cy = if selector == b'c' {
            ctx.row as i64
        } else {
            i64::from(c.y.saturating_sub(1))
        } * i64::from(ch);
        let rows = ctx.rows as i64 * i64::from(ch);
        let mut affected = Vec::new();
        self.placements.retain(|p| {
            let r = p.rect(cw, ch, 0);
            let x = i64::from(r.x);
            let y = i64::from(r.y);
            let right = x + i64::from(r.width);
            let bottom = y + i64::from(r.height);
            let visible =
                bottom > 0 && y < rows && x < ctx.cols as i64 * i64::from(cw) && right > 0;
            let hit_x = x < cx + i64::from(cw) && right > cx;
            let hit_y = y < cy + i64::from(ch) && bottom > cy;
            let delete = match selector {
                b'a' => visible,
                b'i' | b'n' => p.image_id == id && (c.placement == 0 || p.id == c.placement),
                b'r' => p.image_id >= c.x && p.image_id <= c.y,
                b'c' | b'p' => visible && hit_x && hit_y,
                b'q' => visible && hit_x && hit_y && p.z == c.z,
                b'x' => visible && hit_x,
                b'y' => visible && hit_y,
                b'z' => visible && p.z == c.z,
                _ => false,
            };
            if delete && !affected.contains(&p.image_id) {
                affected.push(p.image_id);
            }
            !delete
        });
        if c.delete.is_ascii_uppercase() {
            let placements = &self.placements;
            self.images.retain(|image| {
                let selected = match selector {
                    b'i' | b'n' => image.id == id,
                    b'r' => image.id >= c.x && image.id <= c.y,
                    _ => affected.contains(&image.id),
                };
                !selected || placements.iter().any(|p| p.image_id == image.id)
            });
            self.used = self
                .images
                .iter()
                .map(|image| image.image.pixels.capacity())
                .sum();
        }
        self.collect_unreferenced_anonymous();
        self.changed();
        Ok(())
    }

    fn respond(&self, c: &Command, id: u32, message: &str, replies: &mut Vec<u8>) {
        if c.quiet == 2 || (c.quiet == 1 && message == "OK") || (c.id == 0 && c.number == 0) {
            return;
        }
        let mut reply = format!("\x1b_Gi={id}");
        if c.number != 0 {
            reply.push_str(&format!(",I={}", c.number));
        }
        if c.placement != 0 {
            reply.push_str(&format!(",p={}", c.placement));
        }
        reply.push(';');
        reply.push_str(message);
        reply.push_str("\x1b\\");
        append_reply(replies, &reply);
    }

    fn feature_query(&self, c: &Command, replies: &mut Vec<u8>) {
        if c.id == 0 || c.quiet != 0 {
            return;
        }
        let mut reply = format!("\x1b_Gi={}", c.id);
        if matches!(c.feature, b't' | b'T' | b'p' | b'd') {
            reply.push_str(&format!(",e={}", c.feature as char));
            // Echo only supported nondefault values. Unsupported options remain
            // absent, allowing clients to fall back without a false capability.
            for (key, value) in &c.fields {
                if protocol::feature_supported(c.feature, *key, value) {
                    reply.push(',');
                    reply.push(*key as char);
                    reply.push('=');
                    reply.push_str(value);
                }
            }
        }
        reply.push_str(";OK\x1b\\");
        append_reply(replies, &reply);
    }
}

fn append_reply(replies: &mut Vec<u8>, text: &str) {
    if replies.len().saturating_add(text.len()) <= 65536 {
        replies.extend_from_slice(text.as_bytes());
    }
}

#[cfg(test)]
extern crate std;
#[cfg(test)]
mod sixel_tests;
#[cfg(test)]
mod tests;
