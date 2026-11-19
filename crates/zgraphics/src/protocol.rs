use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub(crate) struct Command {
    pub action: u8,
    pub action_specified: bool,
    pub feature: u8,
    pub format: u32,
    pub transport: u8,
    pub compression: u8,
    pub width: u32,
    pub height: u32,
    pub size: u32,
    pub offset: u32,
    pub id: u32,
    pub number: u32,
    pub placement: u32,
    pub more: bool,
    pub quiet: u8,
    pub quiet_override: Option<u8>,
    pub continuation: bool,
    pub unsupported: bool,
    pub no_cursor_move: bool,
    pub x: u32,
    pub y: u32,
    pub width_crop: u32,
    pub height_crop: u32,
    pub columns: u32,
    pub rows: u32,
    pub x_offset: u32,
    pub y_offset: u32,
    pub z: i32,
    pub virtual_placement: bool,
    pub parent_image: u32,
    pub parent_placement: u32,
    pub horizontal: i32,
    pub vertical: i32,
    pub delete: u8,
    pub fields: Vec<(u8, String)>,
}

impl Default for Command {
    fn default() -> Self {
        Self {
            action: b't',
            action_specified: false,
            feature: 0,
            format: 32,
            transport: b'd',
            compression: 0,
            width: 0,
            height: 0,
            size: 0,
            offset: 0,
            id: 0,
            number: 0,
            placement: 0,
            more: false,
            quiet: 0,
            quiet_override: None,
            continuation: true,
            unsupported: false,
            no_cursor_move: false,
            x: 0,
            y: 0,
            width_crop: 0,
            height_crop: 0,
            columns: 0,
            rows: 0,
            x_offset: 0,
            y_offset: 0,
            z: 0,
            virtual_placement: false,
            parent_image: 0,
            parent_placement: 0,
            horizontal: 0,
            vertical: 0,
            delete: b'a',
            fields: Vec::new(),
        }
    }
}

pub(crate) fn parse(body: &[u8]) -> Result<(Command, &[u8]), &'static str> {
    if body.first() != Some(&b'G') || body.len() > 4608 {
        return Err("EINVAL:invalid graphics command");
    }
    let body = &body[1..];
    let split = body.iter().position(|b| *b == b';').unwrap_or(body.len());
    if split > 511 {
        return Err("E2BIG:graphics header too large");
    }
    let header = &body[..split];
    // Only feature queries echo control keys. Ordinary image chunks need no
    // per-key String/Vec allocation, including thousands of m=1 continuations.
    let retain_fields = header
        .split(|b| *b == b',')
        .any(|part| part.starts_with(b"e="));
    let payload = body.get(split + 1..).unwrap_or(&[]);
    if payload.len() > 4096 {
        return Err("E2BIG:graphics chunk too large");
    }
    let mut c = Command::default();
    let mut seen = [false; 128];
    if header.is_empty() {
        c.continuation = false;
    }
    for part in header.split(|b| *b == b',').filter(|part| !part.is_empty()) {
        if part.len() < 3 || part[1] != b'=' || !part[0].is_ascii_alphabetic() {
            return Err("EINVAL:invalid graphics header");
        }
        let key = part[0];
        if seen[key as usize] {
            return Err("EINVAL:duplicate graphics key");
        }
        seen[key as usize] = true;
        let value = core::str::from_utf8(&part[2..]).map_err(|_| "EINVAL:non-ASCII value")?;
        if !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err("EINVAL:invalid graphics value");
        }
        if retain_fields {
            c.fields.push((key, value.to_string()));
        }
        c.continuation &= matches!(key, b'm' | b'q' | b'a');
        let integer = || value.parse::<u32>().map_err(|_| "EINVAL:invalid integer");
        let character = || {
            if value.len() == 1 {
                Ok(value.as_bytes()[0])
            } else {
                Err("EINVAL:expected character")
            }
        };
        match key {
            b'a' => {
                c.action = character()?;
                c.action_specified = true;
            }
            b'e' => c.feature = character()?,
            b'f' => c.format = integer()?,
            b't' => c.transport = character()?,
            b'o' => {
                c.compression = character()?;
                c.unsupported |= c.compression != b'z';
            }
            b's' => c.width = integer()?,
            b'v' => c.height = integer()?,
            b'S' => c.size = integer()?,
            b'O' => c.offset = integer()?,
            b'i' => c.id = integer()?,
            b'I' => c.number = integer()?,
            b'p' => c.placement = integer()?,
            b'm' => {
                let n = integer()?;
                if n > 1 {
                    return Err("EINVAL:invalid chunk flag");
                }
                c.more = n == 1;
            }
            b'q' => {
                let n = integer()?;
                if n > 2 {
                    return Err("EINVAL:invalid quiet flag");
                }
                c.quiet = n as u8;
                c.quiet_override = Some(n as u8);
            }
            b'C' => {
                let n = integer()?;
                if n > 1 {
                    c.unsupported = true;
                }
                c.no_cursor_move = n == 1;
            }
            b'x' => c.x = integer()?,
            b'y' => c.y = integer()?,
            b'w' => c.width_crop = integer()?,
            b'h' => c.height_crop = integer()?,
            b'c' => c.columns = integer()?,
            b'r' => c.rows = integer()?,
            b'X' => c.x_offset = integer()?,
            b'Y' => c.y_offset = integer()?,
            b'z' => c.z = value.parse::<i32>().map_err(|_| "EINVAL:invalid z index")?,
            b'd' => c.delete = character()?,
            b'U' => {
                let n = integer()?;
                if n > 1 {
                    return Err("EINVAL:invalid virtual placement flag");
                }
                c.virtual_placement = n == 1;
            }
            b'P' => c.parent_image = integer()?,
            b'Q' => c.parent_placement = integer()?,
            b'H' => c.horizontal = value.parse::<i32>().map_err(|_| "EINVAL:invalid offset")?,
            b'V' => c.vertical = value.parse::<i32>().map_err(|_| "EINVAL:invalid offset")?,
            b'N' => {
                let _ = integer()?;
            } // Optional usage hint, deliberately advisory.
            _ => c.unsupported = true,
        }
    }
    if c.more && (!payload.len().is_multiple_of(4) || payload.contains(&b'=')) {
        return Err("EINVAL:invalid nonfinal base64 chunk");
    }
    Ok((c, payload))
}

pub(crate) fn feature_supported(action: u8, key: u8, value: &str, external: bool) -> bool {
    let unsigned = value.parse::<u32>().ok();
    let nonzero = unsigned.is_some_and(|v| v != 0);
    let signed = value.parse::<i32>().is_ok_and(|v| v != 0);
    if matches!(action, b't' | b'T' | b'q' | b'f') {
        let transfer = match key {
            b'f' => matches!(unsigned, Some(24 | 100)),
            b't' => external && matches!(value, "f" | "t" | "s"),
            b'o' => value == "z",
            b's' | b'v' | b'S' | b'O' => nonzero,
            _ => false,
        };
        if transfer {
            return true;
        }
    }
    // The same key has different meanings for display, frame loading and
    // animation control. Only advertise values actually consumed by that action.
    match action {
        b'T' | b'p' => match key {
            b'p' | b'x' | b'y' | b'w' | b'h' | b'X' | b'Y' | b'c' | b'r' | b'P' | b'Q' => nonzero,
            b'C' | b'U' => unsigned == Some(1),
            b'z' | b'H' | b'V' => signed,
            _ => false,
        },
        b'f' => match key {
            b'x' | b'y' | b'c' | b'r' | b'Y' => nonzero,
            b'X' => unsigned == Some(1),
            b'z' => signed,
            _ => false,
        },
        b'a' => match key {
            b's' => matches!(unsigned, Some(1..=3)),
            b'v' | b'c' | b'r' => nonzero,
            b'z' => signed,
            _ => false,
        },
        b'c' => match key {
            b'c' | b'r' | b'x' | b'y' | b'w' | b'h' | b'X' | b'Y' => nonzero,
            b'C' => unsigned == Some(1),
            _ => false,
        },
        b'd' => match key {
            b'p' | b'x' | b'y' | b'r' => nonzero,
            b'z' => signed,
            b'd' => {
                value.len() == 1
                    && value != "a"
                    && matches!(
                        value.as_bytes()[0].to_ascii_lowercase(),
                        b'a' | b'i' | b'n' | b'c' | b'p' | b'q' | b'r' | b'x' | b'y' | b'z' | b'f'
                    )
            }
            _ => false,
        },
        _ => false,
    }
}
