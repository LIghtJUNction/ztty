use alloc::{format, string::String};
use core::ffi::CStr;
use zterm::{CursorShape, Theme};

use crate::os;

pub struct Appearance {
    pub font: String,
    pub theme: Theme,
    pub padding: i32,
    pub opacity: u8,
    pub blur: bool,
    pub cursor_color: u32,
    pub cursor_shape: CursorShape,
    pub cursor_blink: bool,
    pub cursor_blink_interval: u64,
    pub selection_foreground: u32,
    pub selection_background: u32,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            font: String::from(default_font()),
            theme: Theme {
                foreground: 0xe1e7f0,
                background: 0x161a23,
                // ANSI order: black, red, green, yellow, blue, magenta, cyan, white.
                palette: [
                    0x3a4355, 0xe88e98, 0xa3c998, 0xe2c685, 0x91b5f1, 0xc4a7e7, 0x83c8cf, 0xcdd6e4,
                    0x768399, 0xffa8b0, 0xbce2ac, 0xf4dca3, 0xb1ccff, 0xdec4fa, 0xa6e1e7, 0xf0f4fa,
                ],
            },
            padding: 16,
            opacity: 92,
            blur: true,
            cursor_color: 0xb1ccff,
            cursor_shape: CursorShape::Underline,
            cursor_blink: true,
            cursor_blink_interval: 500,
            selection_foreground: 0xf0f4fa,
            selection_background: 0x394969,
        }
    }
}

impl Appearance {
    #[cfg(target_os = "linux")]
    pub fn to_json(&self) -> crate::json::Value {
        use crate::json::Value;
        let mut entries = alloc::vec![
            ("font", self.font.clone()),
            ("foreground", format!("#{:06x}", self.theme.foreground)),
            ("background", format!("#{:06x}", self.theme.background)),
            ("window-padding", format!("{}", self.padding)),
            ("background-opacity", format!("{}", self.opacity)),
            ("background-blur", format!("{}", self.blur)),
            ("cursor-color", format!("#{:06x}", self.cursor_color)),
            (
                "cursor-shape",
                String::from(match self.cursor_shape {
                    CursorShape::Block => "block",
                    CursorShape::Underline => "underline",
                    CursorShape::Bar => "bar",
                })
            ),
            ("cursor-blink", format!("{}", self.cursor_blink)),
            (
                "cursor-blink-interval",
                format!("{}", self.cursor_blink_interval)
            ),
            (
                "selection-foreground",
                format!("#{:06x}", self.selection_foreground)
            ),
            (
                "selection-background",
                format!("#{:06x}", self.selection_background)
            ),
        ];
        for (index, color) in self.theme.palette.iter().enumerate() {
            entries.push(("palette", format!("{index}=#{color:06x}")));
        }
        Value::Array(
            entries
                .into_iter()
                .map(|(key, value)| Value::Array(alloc::vec![Value::from(key), Value::from(value)]))
                .collect(),
        )
    }

    #[cfg(target_os = "linux")]
    pub fn from_json(value: &crate::json::Value) -> Result<Self, &'static str> {
        let entries = value.as_array().ok_or("invalid appearance")?;
        if entries.len() > 64 {
            return Err("too many appearance options");
        }
        let mut appearance = Self::default();
        for entry in entries {
            let pair = entry
                .as_array()
                .filter(|p| p.len() == 2)
                .ok_or("invalid appearance entry")?;
            appearance.set(
                pair[0].as_str().ok_or("invalid appearance key")?,
                pair[1].as_str().ok_or("invalid appearance value")?,
            )?;
        }
        Ok(appearance)
    }

    pub fn load(explicit: Option<&CStr>) -> Result<Self, String> {
        let mut appearance = Self::default();
        let default;
        let path = match explicit {
            Some(path) => path,
            None => {
                let Some(path) = os::default_config_path() else {
                    return Ok(appearance);
                };
                default = path;
                &default
            }
        };
        let bytes = match os::read_config(path) {
            Ok(bytes) => bytes,
            Err(2) if explicit.is_none() => return Ok(appearance),
            Err(errno) => {
                return Err(format!(
                    "{}: cannot read config (OS error {errno})",
                    path.to_string_lossy()
                ));
            }
        };
        let contents = core::str::from_utf8(&bytes)
            .map_err(|_| format!("{}: config must be UTF-8", path.to_string_lossy()))?;
        for (index, line) in contents.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let result = line
                .split_once('=')
                .ok_or("expected key = value")
                .and_then(|(key, value)| appearance.set(key.trim(), value.trim()));
            if let Err(error) = result {
                return Err(format!(
                    "{}:{}: {error} ({line})",
                    path.to_string_lossy(),
                    index + 1
                ));
            }
        }
        Ok(appearance)
    }

    pub fn set_font(&mut self, value: &str) -> Result<(), &'static str> {
        if value.is_empty() || value.len() > 1024 || value.contains('\0') {
            return Err("expected a font name of 1..1024 bytes");
        }
        self.font = String::from(value);
        Ok(())
    }

    fn set(&mut self, key: &str, value: &str) -> Result<(), &'static str> {
        let boolean = || match value {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err("expected true or false"),
        };
        let number = |min, max| {
            value
                .parse::<u64>()
                .ok()
                .filter(|n| (min..=max).contains(n))
                .ok_or("integer outside the supported range")
        };
        let color = || rgb(value);
        match key {
            "font" => self.set_font(value)?,
            "foreground" => self.theme.foreground = color()?,
            "background" => self.theme.background = color()?,
            "window-padding" => self.padding = number(0, 128)? as i32,
            "background-opacity" => self.opacity = number(0, 100)? as u8,
            "background-blur" => self.blur = boolean()?,
            "cursor-color" => self.cursor_color = color()?,
            "cursor-blink" => self.cursor_blink = boolean()?,
            "cursor-blink-interval" => self.cursor_blink_interval = number(100, 5000)?,
            "cursor-shape" => {
                self.cursor_shape = match value {
                    "block" => CursorShape::Block,
                    "underline" => CursorShape::Underline,
                    "bar" => CursorShape::Bar,
                    _ => return Err("expected block, underline or bar"),
                }
            }
            "selection-foreground" => self.selection_foreground = color()?,
            "selection-background" => self.selection_background = color()?,
            "palette" => {
                let (index, value) = value
                    .split_once('=')
                    .ok_or("expected palette = INDEX=#RRGGBB")?;
                let index = index
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .filter(|i| *i < 16)
                    .ok_or("palette index must be 0..15")?;
                self.theme.palette[index] = rgb(value.trim())?;
            }
            _ => return Err("unknown appearance option"),
        }
        Ok(())
    }
}

fn rgb(value: &str) -> Result<u32, &'static str> {
    value
        .strip_prefix('#')
        .filter(|s| s.len() == 6 && s.bytes().all(|c| c.is_ascii_hexdigit()))
        .and_then(|s| u32::from_str_radix(s, 16).ok())
        .ok_or("expected #RRGGBB")
}

fn default_font() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "Consolas:size=11"
    }
    #[cfg(target_os = "macos")]
    {
        "Menlo:size=11"
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        "monospace:size=11"
    }
}
