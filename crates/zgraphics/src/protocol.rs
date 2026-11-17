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
            b'U' | b'P' | b'Q' | b'H' | b'V' => c.unsupported |= value != "0",
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

pub(crate) fn feature_supported(action: u8, key: u8, value: &str) -> bool {
    let nonzero = value.parse::<u32>().is_ok_and(|v| v != 0);
    match key {
        b'f' => matches!(action, b't' | b'T') && matches!(value, "24" | "100"),
        b'o' => matches!(action, b't' | b'T') && value == "z",
        b'C' => matches!(action, b'T' | b'p') && value == "1",
        b's' | b'v' | b'S' => matches!(action, b't' | b'T') && nonzero,
        b'c' | b'r' | b'x' | b'y' | b'w' | b'h' | b'X' | b'Y' | b'p' => {
            matches!(action, b'T' | b'p') && nonzero
        }
        b'z' => matches!(action, b'T' | b'p') && value.parse::<i32>().is_ok_and(|v| v != 0),
        b'd' => {
            action == b'd'
                && value.len() == 1
                && value != "a"
                && matches!(
                    value.as_bytes()[0].to_ascii_lowercase(),
                    b'a' | b'i' | b'n' | b'c' | b'p' | b'q' | b'r' | b'x' | b'y' | b'z'
                )
        }
        _ => false,
    }
}
