use alloc::{collections::BTreeMap, ffi::CString, rc::Rc, string::String, vec, vec::Vec};
use core::{
    ffi::{c_char, c_int, c_void},
    ptr::NonNull,
};

#[derive(Clone, Copy, Default, Debug)]
#[repr(C)]
pub struct Metrics {
    pub cell_width: f32,
    pub cell_height: f32,
    pub ascent: f32,
}

#[derive(Clone, Copy, Default, Debug)]
#[repr(C)]
pub struct Glyph {
    pub glyph: u32,
    pub cluster: u32,
    pub x_advance: f32,
    pub y_advance: f32,
    pub x_offset: f32,
    pub y_offset: f32,
}

#[derive(Default)]
#[repr(C)]
struct NativeBitmap {
    pixels: *const u8,
    width: c_int,
    height: c_int,
    pitch: c_int,
    format: c_int,
    left: c_int,
    top: c_int,
    scale: f32,
}

unsafe extern "C" {
    fn ztty_fonts_new(description: *const c_char, metrics: *mut Metrics) -> *mut c_void;
    fn ztty_fonts_free(fonts: *mut c_void);
    fn ztty_fonts_select(fonts: *mut c_void, text: *const u8, len: usize, style: u32) -> c_int;
    fn ztty_fonts_shape(
        fonts: *mut c_void,
        font: u32,
        text: *const u8,
        len: usize,
        output: *mut Glyph,
        capacity: u32,
    ) -> c_int;
    fn ztty_fonts_glyph(
        fonts: *mut c_void,
        font: u32,
        glyph: u32,
        output: *mut NativeBitmap,
    ) -> c_int;
    fn ztty_unicode_script(scalar: u32) -> u32;
}

pub struct Bitmap {
    /// Premultiplied RGBA. Monochrome masks store white multiplied by coverage.
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub left: f32,
    pub top: f32,
    pub scale: f32,
    pub color: bool,
}

pub struct Fonts {
    native: NonNull<c_void>,
    metrics: Metrics,
    ascii: [[Option<u32>; 128]; 4],
    fallback: [BTreeMap<String, u32>; 4],
    fallback_bytes: usize,
    fallback_entries: usize,
    shaped: BTreeMap<(u32, u64), Shaped>,
    shaped_bytes: usize,
}

const SHAPED_BYTES: usize = 512 * 1024;
const SHAPED_ENTRIES: usize = 512;

struct Shaped {
    text: String,
    glyphs: Rc<[Glyph]>,
    bytes: usize,
}

fn text_hash(text: &str) -> u64 {
    // This is only a bounded cache index. A hit also compares the full text;
    // collisions cannot substitute another run's glyphs.
    text.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

impl Fonts {
    pub fn new(description: &str) -> Result<Self, &'static str> {
        let description = CString::new(description).map_err(|_| "font name contains NUL")?;
        let mut metrics = Metrics::default();
        let native = unsafe { ztty_fonts_new(description.as_ptr(), &mut metrics) };
        Ok(Self {
            native: NonNull::new(native).ok_or("cannot open the configured font")?,
            metrics,
            ascii: [[None; 128]; 4],
            fallback: core::array::from_fn(|_| BTreeMap::new()),
            fallback_bytes: 0,
            fallback_entries: 0,
            shaped: BTreeMap::new(),
            shaped_bytes: 0,
        })
    }

    pub fn metrics(&self) -> Metrics {
        self.metrics
    }

    /// Fontconfig selects a face for the complete grapheme, including emoji.
    /// At most 64 opened faces are retained by one font collection.
    pub fn select(&mut self, text: &str, bold: bool, italic: bool) -> Result<u32, &'static str> {
        if text.len() > 16_384 {
            return Err("font selection text exceeds 16 KiB");
        }
        let style = u32::from(bold) | (u32::from(italic) << 1);
        let ascii =
            (text.len() == 1 && text.as_bytes()[0] < 128).then(|| text.as_bytes()[0] as usize);
        if let Some(character) = ascii {
            if let Some(font) = self.ascii[style as usize][character] {
                return Ok(font);
            }
        } else if let Some(font) = self.fallback[style as usize].get(text) {
            return Ok(*font);
        }
        let id =
            unsafe { ztty_fonts_select(self.native.as_ptr(), text.as_ptr(), text.len(), style) };
        let id = u32::try_from(id).map_err(|_| "font selection failed or face budget exhausted")?;
        if let Some(character) = ascii {
            self.ascii[style as usize][character] = Some(id);
        } else {
            if self.fallback_entries >= 2048 || self.fallback_bytes + text.len() > 256 * 1024 {
                for cache in &mut self.fallback {
                    cache.clear();
                }
                self.fallback_bytes = 0;
                self.fallback_entries = 0;
            }
            self.fallback[style as usize].insert(String::from(text), id);
            self.fallback_bytes += text.len();
            self.fallback_entries += 1;
        }
        Ok(id)
    }

    pub fn shape(&mut self, font: u32, text: &str) -> Result<Rc<[Glyph]>, &'static str> {
        if text.len() > 16_384 {
            return Err("shaping text exceeds 16 KiB");
        }
        let key = (font, text_hash(text));
        if let Some(cached) = self.shaped.get(&key)
            && cached.text == text
        {
            return Ok(Rc::clone(&cached.glyphs));
        }
        let capacity = (text.chars().count() * 3 + 8).min(8192);
        let mut glyphs = vec![Glyph::default(); capacity];
        let mut count = unsafe {
            ztty_fonts_shape(
                self.native.as_ptr(),
                font,
                text.as_ptr(),
                text.len(),
                glyphs.as_mut_ptr(),
                glyphs.len() as u32,
            )
        };
        if count == -2 && glyphs.len() < 8192 {
            glyphs.resize(8192, Glyph::default());
            count = unsafe {
                ztty_fonts_shape(
                    self.native.as_ptr(),
                    font,
                    text.as_ptr(),
                    text.len(),
                    glyphs.as_mut_ptr(),
                    glyphs.len() as u32,
                )
            };
        }
        let count = usize::try_from(count).map_err(|_| "font shaping exceeded its glyph budget")?;
        if count > glyphs.len() {
            return Err("invalid native glyph count");
        }
        glyphs.truncate(count);
        let glyphs: Rc<[Glyph]> = glyphs.into();
        let bytes = text.len() + core::mem::size_of_val(&*glyphs);
        if bytes <= SHAPED_BYTES {
            if self.shaped_bytes + bytes > SHAPED_BYTES || self.shaped.len() >= SHAPED_ENTRIES {
                self.shaped.clear();
                self.shaped_bytes = 0;
            }
            if let Some(previous) = self.shaped.insert(
                key,
                Shaped {
                    text: String::from(text),
                    glyphs: Rc::clone(&glyphs),
                    bytes,
                },
            ) {
                self.shaped_bytes -= previous.bytes;
            }
            self.shaped_bytes += bytes;
        }
        Ok(glyphs)
    }

    pub fn shape_cache_bytes(&self) -> usize {
        self.shaped_bytes
    }

    pub fn rasterize(&mut self, font: u32, glyph: u32) -> Result<Bitmap, &'static str> {
        let mut bitmap = NativeBitmap::default();
        if unsafe { ztty_fonts_glyph(self.native.as_ptr(), font, glyph, &mut bitmap) } == 0 {
            return Err("cannot rasterize the shaped glyph");
        }
        let width = u32::try_from(bitmap.width).map_err(|_| "invalid glyph width")?;
        let height = u32::try_from(bitmap.height).map_err(|_| "invalid glyph height")?;
        if width > 1024 || height > 1024 || !bitmap.scale.is_finite() || bitmap.scale <= 0.0 {
            return Err("glyph dimensions exceed the raster budget");
        }
        let pitch = bitmap.pitch.unsigned_abs() as usize;
        let row_bytes = match bitmap.format {
            1 => (width as usize).div_ceil(8),
            2 => width as usize,
            7 => width as usize * 4,
            _ => return Err("unsupported native glyph pixel format"),
        };
        if pitch < row_bytes
            || pitch > 4 * 1024 * 1024
            || (width > 0 && height > 0 && bitmap.pixels.is_null())
        {
            return Err("invalid native glyph storage");
        }
        let mut rgba = vec![0u8; width as usize * height as usize * 4];
        for y in 0..height as usize {
            // FreeType's signed pitch points from one logical row to the next.
            let row = unsafe { bitmap.pixels.offset(bitmap.pitch as isize * y as isize) };
            for x in 0..width as usize {
                let target = &mut rgba[(y * width as usize + x) * 4..][..4];
                if bitmap.format == 7 {
                    let source = unsafe { core::slice::from_raw_parts(row.add(x * 4), 4) };
                    target.copy_from_slice(&[source[2], source[1], source[0], source[3]]);
                } else {
                    let alpha = unsafe {
                        if bitmap.format == 1 {
                            if *row.add(x / 8) & (0x80 >> (x % 8)) != 0 {
                                255
                            } else {
                                0
                            }
                        } else {
                            *row.add(x)
                        }
                    };
                    target.fill(alpha);
                }
            }
        }
        Ok(Bitmap {
            rgba,
            width,
            height,
            left: bitmap.left as f32 * bitmap.scale,
            top: bitmap.top as f32 * bitmap.scale,
            scale: bitmap.scale,
            color: bitmap.format == 7,
        })
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        unsafe { ztty_fonts_free(self.native.as_ptr()) }
    }
}

/// ISO 15924 script tag returned by the installed HarfBuzz Unicode functions.
pub fn script(character: char) -> u32 {
    unsafe { ztty_unicode_script(character as u32) }
}
