//! Incremental DEC sixel decoding. The input is the DCS body, without framing.
//!
//! Coordinates, decoded allocation, input bytes and painting/copying work have
//! separate bounds. A failed stream is discarded; it never reaches the scene.

use alloc::vec::Vec;

use crate::Limits;

pub type Palette = [[u8; 4]; 256];

const TRANSPARENT: u16 = 256;
const BACKGROUND: u16 = 257;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Parse {
    Body,
    Repeat,
    Raster,
    Color,
}

pub(crate) struct Decoder {
    limits: Limits,
    palette: Palette,
    background: [u8; 4],
    empty: u16,
    pixels: Vec<u16>,
    stride: usize,
    capacity_rows: usize,
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    scale_y: u32,
    color: u16,
    parse: Parse,
    params: [u32; 5],
    parameter: usize,
    started: bool,
    received: usize,
    work: usize,
}

pub(crate) struct Decoded {
    pub width: u32,
    pub height: u32,
    pub scale_y: u32,
    pub pixels: Vec<u8>,
    pub palette: Palette,
}

impl Decoder {
    pub fn new(
        limits: Limits,
        aspect: u16,
        transparent: bool,
        background: [u8; 3],
        palette: Option<&Palette>,
    ) -> Self {
        Self {
            limits,
            palette: palette.copied().unwrap_or_else(default_palette),
            background: [background[0], background[1], background[2], 255],
            empty: if transparent { TRANSPARENT } else { BACKGROUND },
            pixels: Vec::new(),
            stride: 0,
            capacity_rows: 0,
            width: 0,
            height: 0,
            x: 0,
            y: 0,
            scale_y: match aspect {
                2 => 5,
                3 | 4 => 3,
                7..=9 => 1,
                _ => 2,
            },
            color: 15,
            parse: Parse::Body,
            params: [0; 5],
            parameter: 0,
            started: false,
            received: 0,
            work: 0,
        }
    }

    pub fn allocated_bytes(&self) -> usize {
        self.pixels.capacity() * core::mem::size_of::<u16>()
    }

    fn spend(&mut self, amount: usize) -> Result<(), &'static str> {
        self.work = self.work.checked_add(amount).ok_or("sixel work overflow")?;
        if self.work > self.limits.max_image_bytes.saturating_mul(8) {
            return Err("sixel painting limit exceeded");
        }
        Ok(())
    }

    fn size(&self, width: u32, height: u32) -> Result<usize, &'static str> {
        if width > self.limits.max_dimension || height > self.limits.max_dimension / self.scale_y {
            return Err("sixel dimensions exceed limit");
        }
        let pixels = (width as usize)
            .checked_mul(height as usize)
            .ok_or("sixel size overflow")?;
        if pixels > self.limits.max_image_bytes.min(self.limits.max_total_bytes) / 4 {
            return Err("sixel image exceeds memory limit");
        }
        Ok(pixels)
    }

    fn ensure(&mut self, width: u32, height: u32) -> Result<(), &'static str> {
        self.size(width, height)?;
        self.width = self.width.max(width);
        self.height = self.height.max(height);
        if width as usize <= self.stride && height as usize <= self.capacity_rows {
            return Ok(());
        }
        // Geometric growth avoids reallocating once per transmitted column.
        // Near the quota use the exact dimensions; the work budget also bounds
        // adversarial repeated reshaping of that final allocation.
        let mut stride = self
            .stride
            .max(width.checked_next_power_of_two().unwrap_or(width) as usize);
        let mut rows = self
            .capacity_rows
            .max(height.checked_next_power_of_two().unwrap_or(height) as usize);
        if self.size(stride as u32, rows as u32).is_err() {
            stride = self.stride.max(width as usize);
            rows = self.capacity_rows.max(height as usize);
        }
        let length = self.size(stride as u32, rows as u32)?;
        self.spend(length.checked_mul(2).ok_or("sixel work overflow")?)?;
        self.pixels
            .try_reserve_exact(length - self.pixels.len())
            .map_err(|_| "sixel allocation failed")?;
        self.pixels.resize(length, self.empty);
        if stride != self.stride {
            for row in (0..self.capacity_rows).rev() {
                let old = row * self.stride;
                let new = row * stride;
                self.pixels.copy_within(old..old + self.stride, new);
                self.pixels[new + self.stride..new + stride].fill(self.empty);
            }
        }
        self.stride = stride;
        self.capacity_rows = rows;
        Ok(())
    }

    fn paint(&mut self, byte: u8, repeat: u32) -> Result<(), &'static str> {
        self.started = true;
        let repeat = repeat.max(1);
        let end = self.x.checked_add(repeat).ok_or("sixel repeat overflow")?;
        if end > self.limits.max_dimension {
            return Err("sixel repeat exceeds width limit");
        }
        let bits = byte - b'?';
        if bits != 0 {
            let height = self
                .y
                .checked_add(8 - bits.leading_zeros())
                .ok_or("sixel height overflow")?;
            self.spend(
                (repeat as usize)
                    .checked_mul(bits.count_ones() as usize)
                    .ok_or("sixel work overflow")?,
            )?;
            self.ensure(end, height)?;
            for bit in 0..6 {
                if bits & (1 << bit) != 0 {
                    let offset = (self.y as usize + bit) * self.stride;
                    self.pixels[offset + self.x as usize..offset + end as usize].fill(self.color);
                }
            }
        }
        self.x = end;
        Ok(())
    }

    fn begin(&mut self, parse: Parse) {
        self.parse = parse;
        self.params.fill(0);
        self.parameter = 0;
    }

    fn attributes(&mut self) -> Result<(), &'static str> {
        match self.parse {
            Parse::Color => {
                let index = self.params[0] as usize;
                if index >= self.palette.len() {
                    return Err("sixel color register exceeds limit");
                }
                match self.parameter {
                    0 => {}
                    4 => {
                        let [_, model, a, b, c] = self.params;
                        self.palette[index] = match model {
                            1 => hls(a.min(360), b.min(100), c.min(100)),
                            2 => [percent(a), percent(b), percent(c), 255],
                            _ => return Err("unsupported sixel color model"),
                        };
                    }
                    _ => return Err("incomplete sixel color definition"),
                }
                self.color = index as u16;
            }
            Parse::Raster => {
                if self.started || !(1..=3).contains(&self.parameter) {
                    return Err("invalid sixel raster attributes");
                }
                let numerator = self.params[0].max(1);
                let denominator = self.params[1].max(1);
                let scale =
                    (u64::from(numerator) + u64::from(denominator / 2)) / u64::from(denominator);
                if scale > 64 {
                    return Err("sixel aspect ratio exceeds limit");
                }
                self.scale_y = (scale as u32).max(1);
                self.size(self.width, self.height)?;
                let width = self.params[2];
                let height = self.params[3];
                // Either declared dimension may be omitted. Keep it as an
                // extent even before pixels supply the missing dimension.
                self.width = self.width.max(width);
                self.height = self.height.max(height);
                if self.width != 0 && self.height != 0 {
                    self.ensure(self.width, self.height)?;
                } else {
                    self.size(self.width, self.height)?;
                }
            }
            _ => {}
        }
        self.parse = Parse::Body;
        Ok(())
    }

    pub fn push(&mut self, byte: u8) -> Result<(), &'static str> {
        self.received = self.received.saturating_add(1);
        if self.received > self.limits.max_transfer_bytes {
            return Err("sixel input exceeds limit");
        }
        // VT permits embedded controls; framing and cancellation belong to the
        // terminal parser. Ignoring one must not end a numeric parameter.
        if !(0x20..0x7f).contains(&byte) {
            return Ok(());
        }
        if self.parse != Parse::Body {
            if byte.is_ascii_digit() {
                self.params[self.parameter] = self.params[self.parameter]
                    .checked_mul(10)
                    .and_then(|v| v.checked_add(u32::from(byte - b'0')))
                    .ok_or("sixel parameter overflow")?;
                return Ok(());
            }
            if byte == b';' && self.parse != Parse::Repeat {
                self.parameter += 1;
                let limit = if self.parse == Parse::Raster { 4 } else { 5 };
                if self.parameter >= limit {
                    return Err("too many sixel parameters");
                }
                return Ok(());
            }
            if self.parse == Parse::Repeat {
                if !(b'?'..=b'~').contains(&byte) {
                    return Err("invalid sixel repeat");
                }
                self.parse = Parse::Body;
                return self.paint(byte, self.params[0]);
            }
            self.attributes()?;
        }
        match byte {
            b'?'..=b'~' => self.paint(byte, 1)?,
            b'!' => self.begin(Parse::Repeat),
            b'"' => self.begin(Parse::Raster),
            b'#' => self.begin(Parse::Color),
            b'$' => self.x = 0,
            b'-' => {
                self.x = 0;
                self.y = self.y.checked_add(6).ok_or("sixel row overflow")?;
                if self.y > self.limits.max_dimension.saturating_add(6) {
                    return Err("sixel row exceeds limit");
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<Decoded, &'static str> {
        if self.parse == Parse::Repeat {
            return Err("incomplete sixel repeat");
        }
        self.attributes()?;
        if self.width != 0 && self.height != 0 {
            self.ensure(self.width, self.height)?;
        }
        let length = self.size(self.width, self.height)? * 4;
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(length)
            .map_err(|_| "sixel allocation failed")?;
        for row in 0..self.height as usize {
            for &index in &self.pixels[row * self.stride..row * self.stride + self.width as usize] {
                pixels.extend_from_slice(match index {
                    TRANSPARENT => &[0, 0, 0, 0],
                    BACKGROUND => &self.background,
                    _ => &self.palette[index as usize],
                });
            }
        }
        Ok(Decoded {
            width: self.width,
            height: self.height,
            scale_y: self.scale_y,
            pixels,
            palette: self.palette,
        })
    }
}

fn percent(value: u32) -> u8 {
    ((value.min(100) * 255 + 50) / 100) as u8
}

fn hls(hue: u32, lightness: u32, saturation: u32) -> [u8; 4] {
    // Fixed-point HSL with DEC's blue-at-zero hue wheel. Keep four decimal
    // places until the final conversion; no floating-point runtime is needed.
    let chroma = (100 - (2 * lightness as i32 - 100).unsigned_abs()) * saturation;
    let sector = (hue + 240) % 360;
    let secondary = chroma * (60 - (sector as i32 % 120 - 60).unsigned_abs()) / 60;
    let base = lightness * 100 - chroma / 2;
    let rgb = match sector / 60 {
        0 => [chroma, secondary, 0],
        1 => [secondary, chroma, 0],
        2 => [0, chroma, secondary],
        3 => [0, secondary, chroma],
        4 => [secondary, 0, chroma],
        _ => [chroma, 0, secondary],
    };
    let convert = |value: u32| (((value + base) * 255 + 5000) / 10000) as u8;
    [convert(rgb[0]), convert(rgb[1]), convert(rgb[2]), 255]
}

fn default_palette() -> Palette {
    let mut palette = [[0, 0, 0, 255]; 256];
    let colors = [
        [0, 0, 0],
        [20, 20, 80],
        [80, 13, 13],
        [20, 80, 20],
        [80, 20, 80],
        [20, 80, 80],
        [80, 80, 20],
        [53, 53, 53],
        [26, 26, 26],
        [33, 33, 60],
        [60, 26, 26],
        [33, 60, 33],
        [60, 33, 60],
        [33, 60, 60],
        [60, 60, 33],
        [80, 80, 80],
    ];
    for (slot, color) in palette.iter_mut().zip(colors) {
        *slot = [percent(color[0]), percent(color[1]), percent(color[2]), 255];
    }
    palette
}
