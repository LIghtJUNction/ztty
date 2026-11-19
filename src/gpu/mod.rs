//! Bounded OpenGL ES renderer shared by the native Linux window backends.
//! FreeType rasterizes glyphs; HarfBuzz shapes complete runs; the GPU composites
//! cached glyphs, colored emoji, rectangles and images into the native surface.

mod egl;
mod font;
mod gl;
mod grid;

use alloc::{collections::BTreeMap, rc::Rc, string::String, vec, vec::Vec};
use core::{
    ffi::{CStr, c_void},
    mem, ptr,
};
pub use egl::{Display, Target};
pub use font::Metrics;
pub use grid::GridStyle;
use unicode_segmentation::UnicodeSegmentation;

const ATLAS_SIZE: u32 = 1024;
const IMAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_VERTICES: usize = 4092;

#[derive(Clone, Copy, Default, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
}

/// A complete terminal grapheme's byte offset and fixed cell allocation.
/// Positions are pixels relative to the row origin, not HarfBuzz advances.
#[derive(Clone, Copy, Debug)]
pub struct TextCell {
    pub byte: u32,
    pub x: f32,
    pub width: f32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

#[derive(Clone, Copy)]
struct CachedGlyph {
    uv: Rect,
    width: f32,
    height: f32,
    left: f32,
    top: f32,
    color: bool,
}

struct ImageTexture {
    texture: u32,
    revision: u64,
    width: u32,
    height: u32,
    last_used: u64,
}

struct TextRun {
    start: usize,
    end: usize,
    font: u32,
    script: u32,
}

pub struct Renderer {
    surface: egl::Surface,
    fonts: font::Fonts,
    width: u32,
    height: u32,
    program: u32,
    viewport_uniform: i32,
    buffer: u32,
    atlas: u32,
    atlas_x: u32,
    atlas_y: u32,
    atlas_row: u32,
    glyphs: BTreeMap<(u32, u32), CachedGlyph>,
    images: BTreeMap<u64, ImageTexture>,
    image_bytes: usize,
    serial: u64,
    vertices: Vec<Vertex>,
    batch_texture: u32,
    driver: String,
}

fn color(rgb: u32, alpha: u8) -> [f32; 4] {
    [
        ((rgb >> 16) & 255) as f32 / 255.0,
        ((rgb >> 8) & 255) as f32 / 255.0,
        (rgb & 255) as f32 / 255.0,
        f32::from(alpha) / 255.0,
    ]
}

fn shader(kind: u32, source: &CStr) -> Result<u32, &'static str> {
    let shader = unsafe { gl::glCreateShader(kind) };
    if shader == 0 {
        return Err("cannot allocate a GPU shader");
    }
    unsafe {
        gl::glShaderSource(shader, 1, &source.as_ptr(), ptr::null());
        gl::glCompileShader(shader);
    }
    let mut compiled = 0;
    unsafe {
        gl::glGetShaderiv(shader, 0x8b81, &mut compiled);
    }
    if compiled == 0 {
        unsafe {
            gl::glDeleteShader(shader);
        }
        return Err("GPU shader compilation failed");
    }
    Ok(shader)
}

fn program() -> Result<u32, &'static str> {
    let vertex = shader(
        0x8b31,
        c"
        attribute vec2 position;
        attribute vec2 texcoord;
        attribute vec4 tint;
        uniform vec2 viewport;
        varying vec2 uv;
        varying vec4 color;
        void main() {
            gl_Position = vec4(position.x * 2.0 / viewport.x - 1.0,
                1.0 - position.y * 2.0 / viewport.y, 0.0, 1.0);
            uv = texcoord; color = tint;
        }",
    )?;
    let fragment = match shader(
        0x8b30,
        c"
        precision mediump float;
        uniform sampler2D image;
        varying vec2 uv;
        varying vec4 color;
        void main() {
            vec4 sample_color = texture2D(image, uv);
            gl_FragColor = vec4(sample_color.rgb * color.rgb * color.a,
                sample_color.a * color.a);
        }",
    ) {
        Ok(value) => value,
        Err(error) => {
            unsafe {
                gl::glDeleteShader(vertex);
            }
            return Err(error);
        }
    };
    let program = unsafe { gl::glCreateProgram() };
    unsafe {
        gl::glAttachShader(program, vertex);
        gl::glAttachShader(program, fragment);
        gl::glBindAttribLocation(program, 0, c"position".as_ptr());
        gl::glBindAttribLocation(program, 1, c"texcoord".as_ptr());
        gl::glBindAttribLocation(program, 2, c"tint".as_ptr());
        gl::glLinkProgram(program);
        gl::glDeleteShader(vertex);
        gl::glDeleteShader(fragment);
    }
    let mut linked = 0;
    unsafe {
        gl::glGetProgramiv(program, 0x8b82, &mut linked);
    }
    if linked == 0 {
        unsafe {
            gl::glDeleteProgram(program);
        }
        return Err("GPU shader linking failed");
    }
    Ok(program)
}

fn texture(width: u32, height: u32, rgba: &[u8]) -> Result<u32, &'static str> {
    if rgba.len() != width as usize * height as usize * 4 {
        return Err("invalid texture byte count");
    }
    let mut texture = 0;
    unsafe {
        gl::glGenTextures(1, &mut texture);
        gl::glBindTexture(gl::TEXTURE_2D, texture);
        gl::glTexParameteri(gl::TEXTURE_2D, 0x2801, 0x2601);
        gl::glTexParameteri(gl::TEXTURE_2D, 0x2800, 0x2601);
        gl::glTexParameteri(gl::TEXTURE_2D, 0x2802, 0x812f);
        gl::glTexParameteri(gl::TEXTURE_2D, 0x2803, 0x812f);
        gl::glPixelStorei(0x0cf5, 1);
        gl::glTexImage2D(
            gl::TEXTURE_2D,
            0,
            gl::RGBA as i32,
            width as i32,
            height as i32,
            0,
            gl::RGBA,
            gl::UNSIGNED_BYTE,
            rgba.as_ptr().cast(),
        );
        if gl::glGetError() != 0 || texture == 0 {
            gl::glDeleteTextures(1, &texture);
            return Err("GPU texture allocation failed");
        }
    }
    Ok(texture)
}

impl Renderer {
    pub fn new(
        display: Rc<Display>,
        target: Target,
        width: u32,
        height: u32,
        font: &str,
    ) -> Result<Self, &'static str> {
        let surface = egl::Surface::new(display, target, width, height)?;
        let fonts = font::Fonts::new(font)?;
        let program = program()?;
        let atlas = match texture(
            ATLAS_SIZE,
            ATLAS_SIZE,
            &vec![0; (ATLAS_SIZE * ATLAS_SIZE * 4) as usize],
        ) {
            Ok(value) => value,
            Err(error) => {
                unsafe {
                    gl::glDeleteProgram(program);
                }
                return Err(error);
            }
        };
        let mut buffer = 0;
        unsafe {
            gl::glGenBuffers(1, &mut buffer);
        }
        let viewport_uniform = unsafe { gl::glGetUniformLocation(program, c"viewport".as_ptr()) };
        let driver = unsafe {
            let value = gl::glGetString(0x1f01);
            if value.is_null() {
                String::from("unknown")
            } else {
                CStr::from_ptr(value.cast()).to_string_lossy().into_owned()
            }
        };
        let mut renderer = Self {
            surface,
            fonts,
            width,
            height,
            program,
            viewport_uniform,
            buffer,
            atlas,
            atlas_x: 2,
            atlas_y: 0,
            atlas_row: 2,
            glyphs: BTreeMap::new(),
            images: BTreeMap::new(),
            image_bytes: 0,
            serial: 0,
            vertices: Vec::with_capacity(MAX_VERTICES),
            batch_texture: atlas,
            driver,
        };
        renderer.white_pixel();
        renderer.configure();
        renderer.check()?;
        Ok(renderer)
    }

    pub fn metrics(&self) -> Metrics {
        self.fonts.metrics()
    }
    pub fn driver(&self) -> &str {
        &self.driver
    }
    pub fn image_cache_bytes(&self) -> usize {
        self.image_bytes
    }
    pub fn glyph_cache_entries(&self) -> usize {
        self.glyphs.len()
    }

    fn check(&self) -> Result<(), &'static str> {
        if unsafe { gl::glGetError() } == 0 {
            Ok(())
        } else {
            Err("OpenGL ES rejected a rendering operation")
        }
    }

    fn configure(&mut self) {
        unsafe {
            gl::glViewport(0, 0, self.width as i32, self.height as i32);
            gl::glUseProgram(self.program);
            gl::glUniform2f(self.viewport_uniform, self.width as f32, self.height as f32);
            gl::glUniform1i(gl::glGetUniformLocation(self.program, c"image".as_ptr()), 0);
            gl::glActiveTexture(0x84c0);
            gl::glBindBuffer(gl::ARRAY_BUFFER, self.buffer);
            gl::glEnableVertexAttribArray(0);
            gl::glEnableVertexAttribArray(1);
            gl::glEnableVertexAttribArray(2);
            gl::glVertexAttribPointer(
                0,
                2,
                gl::FLOAT,
                0,
                mem::size_of::<Vertex>() as i32,
                ptr::null(),
            );
            gl::glVertexAttribPointer(
                1,
                2,
                gl::FLOAT,
                0,
                mem::size_of::<Vertex>() as i32,
                (2 * mem::size_of::<f32>()) as *const c_void,
            );
            gl::glVertexAttribPointer(
                2,
                4,
                gl::FLOAT,
                0,
                mem::size_of::<Vertex>() as i32,
                (4 * mem::size_of::<f32>()) as *const c_void,
            );
            gl::glEnable(0x0be2);
            gl::glBlendFunc(1, 0x0303);
        }
    }

    fn white_pixel(&mut self) {
        unsafe {
            gl::glBindTexture(gl::TEXTURE_2D, self.atlas);
            gl::glTexSubImage2D(
                gl::TEXTURE_2D,
                0,
                0,
                0,
                1,
                1,
                gl::RGBA,
                gl::UNSIGNED_BYTE,
                [255u8; 4].as_ptr().cast(),
            );
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), &'static str> {
        self.surface.activate()?;
        self.flush()?;
        self.surface.resize(width, height)?;
        self.surface.activate()?;
        self.width = width;
        self.height = height;
        self.configure();
        self.check()
    }

    /// Begin a complete frame. The caller retains terminal damage decisions;
    /// hidden/idle views do not call this function or schedule repaint polling.
    pub fn begin_frame(&mut self, background: u32, alpha: u8) -> Result<(), &'static str> {
        self.surface.activate()?;
        self.vertices.clear();
        self.configure();
        let c = color(background, alpha);
        unsafe {
            gl::glDisable(0x0c11);
            gl::glClearColor(c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]);
            gl::glClear(0x00004000);
        }
        self.serial = self.serial.wrapping_add(1);
        Ok(())
    }

    pub fn clip(&mut self, rectangle: Option<Rect>) -> Result<(), &'static str> {
        self.flush()?;
        unsafe {
            if let Some(r) = rectangle {
                let left = (r.x as i32).clamp(0, self.width as i32);
                let top = (r.y as i32).clamp(0, self.height as i32);
                let right = ((r.x + r.width) as i32).clamp(left, self.width as i32);
                let bottom = ((r.y + r.height) as i32).clamp(top, self.height as i32);
                gl::glEnable(0x0c11);
                gl::glScissor(
                    left,
                    self.height as i32 - bottom,
                    right - left,
                    bottom - top,
                );
            } else {
                gl::glDisable(0x0c11);
            }
        }
        Ok(())
    }

    fn quad(
        &mut self,
        rectangle: Rect,
        uv: Rect,
        tint: [f32; 4],
        texture: u32,
    ) -> Result<(), &'static str> {
        if rectangle.width <= 0.0 || rectangle.height <= 0.0 {
            return Ok(());
        }
        if texture != self.batch_texture || self.vertices.len() + 6 > MAX_VERTICES {
            self.flush()?;
            self.batch_texture = texture;
        }
        let vertex = |x: f32, y: f32, u: f32, v: f32| Vertex {
            position: [x, y],
            uv: [u, v],
            color: tint,
        };
        let x = rectangle.x;
        let y = rectangle.y;
        let right = x + rectangle.width;
        let bottom = y + rectangle.height;
        let u = uv.x;
        let v = uv.y;
        let ur = u + uv.width;
        let vb = v + uv.height;
        self.vertices.extend_from_slice(&[
            vertex(x, y, u, v),
            vertex(right, y, ur, v),
            vertex(x, bottom, u, vb),
            vertex(x, bottom, u, vb),
            vertex(right, y, ur, v),
            vertex(right, bottom, ur, vb),
        ]);
        Ok(())
    }

    pub fn draw_rect(&mut self, rectangle: Rect, rgb: u32, alpha: u8) -> Result<(), &'static str> {
        let middle = 0.5 / ATLAS_SIZE as f32;
        self.quad(
            rectangle,
            Rect {
                x: middle,
                y: middle,
                width: 0.0,
                height: 0.0,
            },
            color(rgb, alpha),
            self.atlas,
        )
    }

    fn flush(&mut self) -> Result<(), &'static str> {
        if !self.surface.is_current() {
            self.surface.activate()?;
            self.configure();
        }
        if self.vertices.is_empty() {
            return Ok(());
        }
        unsafe {
            gl::glBindTexture(gl::TEXTURE_2D, self.batch_texture);
            gl::glBindBuffer(gl::ARRAY_BUFFER, self.buffer);
            gl::glBufferData(
                gl::ARRAY_BUFFER,
                (self.vertices.len() * mem::size_of::<Vertex>()) as isize,
                self.vertices.as_ptr().cast(),
                0x88e0,
            );
            gl::glDrawArrays(4, 0, self.vertices.len() as i32);
        }
        self.vertices.clear();
        Ok(())
    }

    fn clear_atlas(&mut self) -> Result<(), &'static str> {
        self.flush()?;
        let zero = vec![0u8; (ATLAS_SIZE * ATLAS_SIZE * 4) as usize];
        unsafe {
            gl::glBindTexture(gl::TEXTURE_2D, self.atlas);
            gl::glTexSubImage2D(
                gl::TEXTURE_2D,
                0,
                0,
                0,
                ATLAS_SIZE as i32,
                ATLAS_SIZE as i32,
                gl::RGBA,
                gl::UNSIGNED_BYTE,
                zero.as_ptr().cast(),
            );
        }
        self.glyphs.clear();
        self.atlas_x = 2;
        self.atlas_y = 0;
        self.atlas_row = 2;
        self.white_pixel();
        Ok(())
    }

    fn glyph(&mut self, font: u32, glyph: u32) -> Result<CachedGlyph, &'static str> {
        if let Some(cached) = self.glyphs.get(&(font, glyph)) {
            return Ok(*cached);
        }
        let bitmap = self.fonts.rasterize(font, glyph)?;
        if bitmap.width + 2 > ATLAS_SIZE || bitmap.height + 2 > ATLAS_SIZE {
            return Err("glyph exceeds the fixed GPU atlas dimensions");
        }
        if self.atlas_x + bitmap.width + 2 > ATLAS_SIZE {
            self.atlas_x = 0;
            self.atlas_y += self.atlas_row;
            self.atlas_row = 0;
        }
        if self.atlas_y + bitmap.height + 2 > ATLAS_SIZE || self.glyphs.len() >= 4096 {
            self.clear_atlas()?;
        }
        let x = self.atlas_x + 1;
        let y = self.atlas_y + 1;
        // A pending draw may still reference this texture. Flush before changing
        // its contents so the old atlas cannot be observed with new coordinates.
        self.flush()?;
        if bitmap.width > 0 && bitmap.height > 0 {
            unsafe {
                gl::glBindTexture(gl::TEXTURE_2D, self.atlas);
                gl::glTexSubImage2D(
                    gl::TEXTURE_2D,
                    0,
                    x as i32,
                    y as i32,
                    bitmap.width as i32,
                    bitmap.height as i32,
                    gl::RGBA,
                    gl::UNSIGNED_BYTE,
                    bitmap.rgba.as_ptr().cast(),
                );
            }
        }
        let cached = CachedGlyph {
            uv: Rect {
                x: x as f32 / ATLAS_SIZE as f32,
                y: y as f32 / ATLAS_SIZE as f32,
                width: bitmap.width as f32 / ATLAS_SIZE as f32,
                height: bitmap.height as f32 / ATLAS_SIZE as f32,
            },
            width: bitmap.width as f32 * bitmap.scale,
            height: bitmap.height as f32 * bitmap.scale,
            left: bitmap.left,
            top: bitmap.top,
            color: bitmap.color,
        };
        self.atlas_x += bitmap.width + 2;
        self.atlas_row = self.atlas_row.max(bitmap.height + 2);
        self.glyphs.insert((font, glyph), cached);
        Ok(cached)
    }

    fn runs(&mut self, text: &str, style: Style) -> Result<Vec<TextRun>, &'static str> {
        if text.len() > 16_384 {
            return Err("one shaping run is limited to 16 KiB");
        }
        let mut runs: Vec<TextRun> = Vec::new();
        for (start, grapheme) in text.grapheme_indices(true) {
            let font = self.fonts.select(grapheme, style.bold, style.italic)?;
            let script = grapheme
                .chars()
                .map(font::script)
                .find(|script| !matches!(*script, 0x5a797979 | 0x5a696e68 | 0x5a7a7a7a))
                .unwrap_or(0);
            if let Some(last) = runs.last_mut()
                && last.font == font
                && (script == 0 || last.script == 0 || last.script == script)
            {
                last.end = start + grapheme.len();
                if last.script == 0 {
                    last.script = script;
                }
                continue;
            }
            runs.push(TextRun {
                start,
                end: start + grapheme.len(),
                font,
                script,
            });
        }
        Ok(runs)
    }

    fn draw_glyph(
        &mut self,
        font: u32,
        glyph: u32,
        x: f32,
        baseline: f32,
        horizontal_scale: f32,
        rgb: u32,
    ) -> Result<(), &'static str> {
        let cached = self.glyph(font, glyph)?;
        self.quad(
            Rect {
                x: x + cached.left * horizontal_scale,
                y: baseline - cached.top,
                width: cached.width * horizontal_scale,
                height: cached.height,
            },
            cached.uv,
            if cached.color {
                [1.0; 4]
            } else {
                color(rgb, 255)
            },
            self.atlas,
        )?;
        Ok(())
    }

    /// Shape free-flowing UI text with script and complete-grapheme font fallback.
    pub fn draw_text(
        &mut self,
        text: &str,
        x: f32,
        baseline: f32,
        rgb: u32,
        style: Style,
    ) -> Result<f32, &'static str> {
        let mut pen = x;
        for run in self.runs(text, style)? {
            let glyphs = self.fonts.shape(run.font, &text[run.start..run.end])?;
            for glyph in glyphs {
                self.draw_glyph(
                    run.font,
                    glyph.glyph,
                    pen + glyph.x_offset,
                    baseline - glyph.y_offset,
                    1.0,
                    rgb,
                )?;
                pen += glyph.x_advance;
            }
        }
        Ok(pen - x)
    }

    /// Shape terminal runs while keeping each cluster within its assigned cells.
    /// The caller groups equal foreground/style and supplies the unmodified text.
    pub fn draw_cells(
        &mut self,
        text: &str,
        cells: &[TextCell],
        x: f32,
        baseline: f32,
        rgb: u32,
        style: Style,
    ) -> Result<(), &'static str> {
        if text.is_empty() {
            return Ok(());
        }
        if cells.is_empty()
            || cells[0].byte != 0
            || cells.len() > 8192
            || cells.windows(2).any(|pair| pair[0].byte >= pair[1].byte)
            || cells.iter().any(|cell| {
                !text.is_char_boundary(cell.byte as usize)
                    || cell.byte as usize >= text.len()
                    || !cell.x.is_finite()
                    || !cell.width.is_finite()
                    || cell.width < 0.0
            })
        {
            return Err("invalid shaped-cell mapping");
        }
        for run in self.runs(text, style)? {
            let glyphs = self.fonts.shape(run.font, &text[run.start..run.end])?;
            let mut index = 0;
            while index < glyphs.len() {
                let cluster = glyphs[index].cluster as usize + run.start;
                if cluster >= run.end || !text.is_char_boundary(cluster) {
                    return Err("invalid HarfBuzz cluster offset");
                }
                let mut end = index + 1;
                while end < glyphs.len() && glyphs[end].cluster == glyphs[index].cluster {
                    end += 1;
                }
                let next = glyphs
                    .iter()
                    .map(|glyph| glyph.cluster as usize + run.start)
                    .filter(|byte| *byte > cluster)
                    .min()
                    .unwrap_or(run.end);
                let first = cells
                    .partition_point(|cell| cell.byte as usize <= cluster)
                    .saturating_sub(1);
                let last = cells
                    .partition_point(|cell| (cell.byte as usize) < next)
                    .saturating_sub(1)
                    .max(first);
                let width = cells[last].x + cells[last].width - cells[first].x;
                let advance: f32 = glyphs[index..end].iter().map(|glyph| glyph.x_advance).sum();
                let scale = if advance.abs() > 0.01 && width > 0.0 {
                    width / advance.abs()
                } else {
                    1.0
                };
                let mut pen = 0.0;
                for glyph in &glyphs[index..end] {
                    self.draw_glyph(
                        run.font,
                        glyph.glyph,
                        x + cells[first].x + (pen + glyph.x_offset) * scale,
                        baseline - glyph.y_offset,
                        scale,
                        rgb,
                    )?;
                    pen += glyph.x_advance;
                }
                index = end;
            }
        }
        Ok(())
    }

    pub fn draw_image(
        &mut self,
        key: u64,
        revision: u64,
        size: (u32, u32),
        rgba: &[u8],
        source: Rect,
        destination: Rect,
    ) -> Result<(), &'static str> {
        let (width, height) = size;
        let bytes = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .filter(|n| *n <= IMAGE_BYTES && *n == rgba.len())
            .ok_or("image exceeds the GPU cache budget")?;
        if width == 0 || height == 0 || width > 8192 || height > 8192 {
            return Err("invalid GPU image dimensions");
        }
        let stale = self.images.get(&key).is_some_and(|entry| {
            entry.revision != revision || entry.width != width || entry.height != height
        });
        if stale {
            self.remove_image(key)?;
        }
        if !self.images.contains_key(&key) {
            self.flush()?;
            while self.image_bytes + bytes > IMAGE_BYTES || self.images.len() >= 64 {
                let victim = self
                    .images
                    .iter()
                    .min_by_key(|(_, image)| image.last_used)
                    .map(|(key, _)| *key)
                    .ok_or("GPU image cache cannot admit the image")?;
                self.remove_image(victim)?;
            }
            let mut premultiplied = rgba.to_vec();
            for pixel in premultiplied.as_chunks_mut::<4>().0 {
                let alpha = u16::from(pixel[3]);
                for channel in &mut pixel[..3] {
                    *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
                }
            }
            let texture = texture(width, height, &premultiplied)?;
            self.images.insert(
                key,
                ImageTexture {
                    texture,
                    revision,
                    width,
                    height,
                    last_used: self.serial,
                },
            );
            self.image_bytes += bytes;
        }
        let entry = self.images.get_mut(&key).unwrap();
        entry.last_used = self.serial;
        let texture = entry.texture;
        self.quad(
            destination,
            Rect {
                x: source.x / width as f32,
                y: source.y / height as f32,
                width: source.width / width as f32,
                height: source.height / height as f32,
            },
            [1.0; 4],
            texture,
        )?;
        Ok(())
    }

    pub fn remove_image(&mut self, key: u64) -> Result<(), &'static str> {
        if self.images.contains_key(&key) {
            self.flush()?;
        }
        if let Some(image) = self.images.remove(&key) {
            self.image_bytes -= image.width as usize * image.height as usize * 4;
            unsafe {
                gl::glDeleteTextures(1, &image.texture);
            }
        }
        Ok(())
    }

    pub fn clear_images(&mut self) -> Result<(), &'static str> {
        self.flush()?;
        for image in self.images.values() {
            unsafe {
                gl::glDeleteTextures(1, &image.texture);
            }
        }
        self.images.clear();
        self.image_bytes = 0;
        Ok(())
    }

    pub fn present(&mut self) -> Result<(), &'static str> {
        self.flush()?;
        self.check()?;
        self.surface.present()
    }

    /// Complete readback for private rendering fixtures, in top-to-bottom RGBA.
    pub fn read_pixels(&mut self) -> Result<Vec<u8>, &'static str> {
        self.flush()?;
        let mut bytes = vec![0; self.width as usize * self.height as usize * 4];
        unsafe {
            gl::glPixelStorei(0x0d05, 1);
            gl::glReadPixels(
                0,
                0,
                self.width as i32,
                self.height as i32,
                gl::RGBA,
                gl::UNSIGNED_BYTE,
                bytes.as_mut_ptr().cast(),
            );
        }
        self.check()?;
        let pitch = self.width as usize * 4;
        for y in 0..self.height as usize / 2 {
            let opposite = self.height as usize - y - 1;
            let (prefix, suffix) = bytes.split_at_mut(opposite * pitch);
            prefix[y * pitch..(y + 1) * pitch].swap_with_slice(&mut suffix[..pitch]);
        }
        Ok(bytes)
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        if self.surface.activate().is_err() {
            return;
        }
        unsafe {
            for image in self.images.values() {
                gl::glDeleteTextures(1, &image.texture);
            }
            gl::glDeleteTextures(1, &self.atlas);
            gl::glDeleteBuffers(1, &self.buffer);
            gl::glDeleteProgram(self.program);
        }
    }
}

#[cfg(test)]
mod tests;
