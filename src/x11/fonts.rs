//! Shared font faces and per-window color lookup.
use super::*;

impl Connection {
    pub(super) fn fonts(self: &Rc<Self>, name: &str) -> Result<Rc<Fonts>, &'static str> {
        let mut cache = self.fonts.borrow_mut();
        cache.retain(|font| font.strong_count() != 0);
        for font in cache.iter().filter_map(Weak::upgrade) {
            if font.name == name {
                return Ok(font);
            }
        }
        let c_name = CString::new(name).map_err(|_| "invalid font name")?;
        let font = unsafe { XftFontOpenName(self.display, self.screen, c_name.as_ptr()) };
        if font.is_null() {
            return Err("cannot load the requested font");
        }
        let fonts = Rc::new(Fonts {
            connection: Rc::clone(self),
            name: name.into(),
            font,
            fallbacks: RefCell::new(Vec::new()),
            italic: RefCell::new(Vec::new()),
        });
        cache.push(Rc::downgrade(&fonts));
        Ok(fonts)
    }
}

impl Fonts {
    pub(super) fn italic_glyph(&self, ch: char) -> *mut Font {
        let display = self.connection.display;
        let mut fonts = self.italic.borrow_mut();
        unsafe {
            for &font in fonts.iter() {
                if XftCharExists(display, font, ch as u32) != 0 {
                    return font;
                }
            }
            if fonts.len() < 16
                && let Ok(name) =
                    CString::new(format!("{}:slant=100:charset={:x}", self.name, ch as u32))
            {
                let font = XftFontOpenName(display, self.connection.screen, name.as_ptr());
                if !font.is_null() {
                    if XftCharExists(display, font, ch as u32) != 0 {
                        fonts.push(font);
                        return font;
                    }
                    XftFontClose(display, font);
                }
            }
        }
        self.glyph(ch)
    }

    fn glyph(&self, ch: char) -> *mut Font {
        let display = self.connection.display;
        unsafe {
            if ch.is_ascii() || XftCharExists(display, self.font, ch as u32) != 0 {
                return self.font;
            }
            let mut fallbacks = self.fallbacks.borrow_mut();
            for &font in fallbacks.iter() {
                if XftCharExists(display, font, ch as u32) != 0 {
                    return font;
                }
            }
            if fallbacks.len() < 16
                && let Ok(name) = CString::new(format!("{}:charset={:x}", self.name, ch as u32))
            {
                let font = XftFontOpenName(display, self.connection.screen, name.as_ptr());
                if !font.is_null() {
                    if XftCharExists(display, font, ch as u32) != 0 {
                        fallbacks.push(font);
                        return font;
                    }
                    XftFontClose(display, font);
                }
            }
        }
        self.font
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        unsafe {
            for &font in self.fallbacks.get_mut().iter() {
                XftFontClose(self.connection.display, font);
            }
            for &font in self.italic.get_mut().iter() {
                XftFontClose(self.connection.display, font);
            }
            XftFontClose(self.connection.display, self.font);
        }
    }
}

impl Window {
    pub(super) fn color(&mut self, rgb: u32) -> Color {
        if let Some(c) = self.colors.iter().find(|c| c.rgb == rgb) {
            return c.color;
        }
        let value = RenderColor {
            red: (((rgb >> 16) & 255) * 257) as u16,
            green: (((rgb >> 8) & 255) * 257) as u16,
            blue: ((rgb & 255) * 257) as u16,
            alpha: 65535,
        };
        let mut color = Color {
            pixel: rgb as c_ulong,
            value,
        };
        unsafe {
            XftColorAllocValue(
                self.state.display,
                self.visual,
                self.colormap,
                &value,
                &mut color,
            );
        }
        if self.colors.len() < 256 {
            self.colors.push(ColorEntry { rgb, color });
        } else {
            let entry = &mut self.colors[self.next_color];
            unsafe {
                XftColorFree(
                    self.state.display,
                    self.visual,
                    self.colormap,
                    &mut entry.color,
                );
            }
            *entry = ColorEntry { rgb, color };
            self.next_color = (self.next_color + 1) % 256;
        }
        color
    }
    pub(super) fn glyph_font(&mut self, ch: char) -> *mut Font {
        self.fonts.glyph(ch)
    }
}
