//! Bounded JSON values shared by the local session protocol and MCP.
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub const MAX_MESSAGE: usize = 1024 * 1024;
const MAX_DEPTH: usize = 64;
const MAX_VALUES: usize = 65536;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// The original JSON token, without rounding large request identifiers.
    Number(String),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(fields) => fields.iter().find(|(name, _)| name == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn set(&mut self, key: &str, value: Self) {
        if let Self::Object(fields) = self {
            if let Some((_, old)) = fields.iter_mut().find(|(name, _)| name == key) {
                *old = value;
            } else {
                fields.push((key.into(), value));
            }
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        if let Self::String(value) = self {
            Some(value)
        } else {
            None
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        if let Self::Number(value) = self {
            value.parse().ok()
        } else {
            None
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        if let Self::Number(value) = self {
            value.parse().ok()
        } else {
            None
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        if let Self::Bool(value) = self {
            Some(*value)
        } else {
            None
        }
    }

    pub fn as_array(&self) -> Option<&[Self]> {
        if let Self::Array(value) = self {
            Some(value)
        } else {
            None
        }
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::String(value.into())
    }
}
impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}
impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}
macro_rules! numbers {
    ($($number:ty),*) => {$(
        impl From<$number> for Value {
            fn from(value: $number) -> Self { Self::Number(value.to_string()) }
        }
    )*};
}
numbers!(usize, u64, u32, u16, i32, i64);

pub fn object<const N: usize>(fields: [(&str, Value); N]) -> Value {
    Value::Object(
        fields
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
    )
}

pub fn parse(bytes: &[u8]) -> Result<Value, &'static str> {
    if bytes.len() > MAX_MESSAGE {
        return Err("JSON message exceeds 1 MiB");
    }
    parse_checkpoint(bytes)
}

/// Decode private, sealed runtime checkpoint metadata. Network and public IPC
/// messages continue to use `parse` and its independent 1 MiB limit.
pub fn parse_checkpoint(bytes: &[u8]) -> Result<Value, &'static str> {
    if bytes.len() > 32 * 1024 * 1024 {
        return Err("checkpoint JSON exceeds 32 MiB");
    }
    core::str::from_utf8(bytes).map_err(|_| "JSON is not UTF-8")?;
    let mut parser = Parser {
        bytes,
        position: 0,
        values: 0,
    };
    let value = parser.value(0)?;
    parser.whitespace();
    if parser.position != bytes.len() {
        return Err("trailing JSON data");
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    position: usize,
    values: usize,
}

impl Parser<'_> {
    fn whitespace(&mut self) {
        while self
            .bytes
            .get(self.position)
            .is_some_and(|b| matches!(b, b' ' | b'\n' | b'\r' | b'\t'))
        {
            self.position += 1;
        }
    }

    fn take(&mut self, byte: u8) -> bool {
        if self.bytes.get(self.position) == Some(&byte) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, &'static str> {
        if depth > MAX_DEPTH {
            return Err("JSON nesting exceeds 64 levels");
        }
        self.values += 1;
        if self.values > MAX_VALUES {
            return Err("too many JSON values");
        }
        self.whitespace();
        match self.bytes.get(self.position).copied() {
            Some(b'n') => self.literal(b"null", Value::Null),
            Some(b't') => self.literal(b"true", Value::Bool(true)),
            Some(b'f') => self.literal(b"false", Value::Bool(false)),
            Some(b'"') => self.string().map(Value::String),
            Some(b'[') => {
                self.position += 1;
                self.whitespace();
                let mut items = Vec::new();
                if self.take(b']') {
                    return Ok(Value::Array(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.whitespace();
                    if self.take(b']') {
                        return Ok(Value::Array(items));
                    }
                    if !self.take(b',') {
                        return Err("expected comma in array");
                    }
                }
            }
            Some(b'{') => {
                self.position += 1;
                self.whitespace();
                let mut fields = Vec::new();
                if self.take(b'}') {
                    return Ok(Value::Object(fields));
                }
                loop {
                    self.whitespace();
                    let key = self.string()?;
                    self.whitespace();
                    if !self.take(b':') {
                        return Err("expected colon in object");
                    }
                    fields.push((key, self.value(depth + 1)?));
                    self.whitespace();
                    if self.take(b'}') {
                        unique_keys(&fields)?;
                        return Ok(Value::Object(fields));
                    }
                    if !self.take(b',') {
                        return Err("expected comma in object");
                    }
                }
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err("expected JSON value"),
        }
    }

    fn literal(&mut self, token: &[u8], value: Value) -> Result<Value, &'static str> {
        if self.bytes.get(self.position..self.position + token.len()) != Some(token) {
            return Err("invalid JSON literal");
        }
        self.position += token.len();
        Ok(value)
    }

    fn number(&mut self) -> Result<Value, &'static str> {
        let start = self.position;
        self.take(b'-');
        if !self.take(b'0') {
            if !self
                .bytes
                .get(self.position)
                .is_some_and(|b| matches!(b, b'1'..=b'9'))
            {
                return Err("invalid JSON number");
            }
            self.position += 1;
            while self
                .bytes
                .get(self.position)
                .is_some_and(u8::is_ascii_digit)
            {
                self.position += 1;
            }
        }
        if self.take(b'.') {
            let digits = self.position;
            while self
                .bytes
                .get(self.position)
                .is_some_and(u8::is_ascii_digit)
            {
                self.position += 1;
            }
            if self.position == digits {
                return Err("missing fraction");
            }
        }
        if self.take(b'e') || self.take(b'E') {
            if !self.take(b'+') {
                self.take(b'-');
            }
            let digits = self.position;
            while self
                .bytes
                .get(self.position)
                .is_some_and(u8::is_ascii_digit)
            {
                self.position += 1;
            }
            if self.position == digits {
                return Err("missing exponent");
            }
        }
        let number = core::str::from_utf8(&self.bytes[start..self.position])
            .map_err(|_| "invalid number")?;
        Ok(Value::Number(number.into()))
    }

    fn hex4(&mut self) -> Result<u32, &'static str> {
        let mut code = 0;
        for _ in 0..4 {
            let byte = *self
                .bytes
                .get(self.position)
                .ok_or("incomplete Unicode escape")?;
            self.position += 1;
            code = code * 16
                + match byte {
                    b'0'..=b'9' => u32::from(byte - b'0'),
                    b'a'..=b'f' => u32::from(byte - b'a' + 10),
                    b'A'..=b'F' => u32::from(byte - b'A' + 10),
                    _ => return Err("invalid Unicode escape"),
                };
        }
        Ok(code)
    }

    fn string(&mut self) -> Result<String, &'static str> {
        if !self.take(b'"') {
            return Err("expected JSON string");
        }
        let mut output = String::new();
        loop {
            let start = self.position;
            while self
                .bytes
                .get(self.position)
                .is_some_and(|b| *b >= 0x20 && *b != b'"' && *b != b'\\')
            {
                self.position += 1;
            }
            output.push_str(
                core::str::from_utf8(&self.bytes[start..self.position])
                    .map_err(|_| "invalid string")?,
            );
            match self.bytes.get(self.position).copied() {
                Some(b'"') => {
                    self.position += 1;
                    return Ok(output);
                }
                Some(b'\\') => {
                    self.position += 1;
                    let escape = *self.bytes.get(self.position).ok_or("incomplete escape")?;
                    self.position += 1;
                    match escape {
                        b'"' => output.push('"'),
                        b'\\' => output.push('\\'),
                        b'/' => output.push('/'),
                        b'b' => output.push('\u{8}'),
                        b'f' => output.push('\u{c}'),
                        b'n' => output.push('\n'),
                        b'r' => output.push('\r'),
                        b't' => output.push('\t'),
                        b'u' => {
                            let high = self.hex4()?;
                            let scalar = if (0xd800..=0xdbff).contains(&high) {
                                if !self.take(b'\\') || !self.take(b'u') {
                                    return Err("missing low surrogate");
                                }
                                let low = self.hex4()?;
                                if !(0xdc00..=0xdfff).contains(&low) {
                                    return Err("invalid low surrogate");
                                }
                                0x10000 + ((high - 0xd800) << 10) + low - 0xdc00
                            } else {
                                high
                            };
                            output.push(char::from_u32(scalar).ok_or("invalid Unicode scalar")?);
                        }
                        _ => return Err("invalid JSON escape"),
                    }
                }
                Some(_) => return Err("unescaped control character"),
                None => return Err("unterminated JSON string"),
            }
        }
    }
}

/// Preserve object order without quadratic work for attacker-controlled objects.
/// Most messages have only a few keys and need no temporary allocation.
fn unique_keys(fields: &[(String, Value)]) -> Result<(), &'static str> {
    let duplicate = if fields.len() <= 32 {
        fields
            .iter()
            .enumerate()
            .any(|(index, (key, _))| fields[..index].iter().any(|(previous, _)| previous == key))
    } else {
        let mut order: Vec<usize> = (0..fields.len()).collect();
        order.sort_unstable_by(|left, right| fields[*left].0.cmp(&fields[*right].0));
        order
            .windows(2)
            .any(|pair| fields[pair[0]].0 == fields[pair[1]].0)
    };
    if duplicate {
        Err("duplicate JSON object key")
    } else {
        Ok(())
    }
}

pub fn stringify(value: &Value) -> String {
    let mut output = String::new();
    encode(value, &mut output);
    output
}

fn encode(value: &Value, output: &mut String) {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(true) => output.push_str("true"),
        Value::Bool(false) => output.push_str("false"),
        Value::Number(value) => output.push_str(value),
        Value::String(value) => {
            output.push('"');
            for character in value.chars() {
                match character {
                    '"' => output.push_str("\\\""),
                    '\\' => output.push_str("\\\\"),
                    '\n' => output.push_str("\\n"),
                    '\r' => output.push_str("\\r"),
                    '\t' => output.push_str("\\t"),
                    c if c < ' ' => output.push_str(&format!("\\u{:04x}", c as u32)),
                    c => output.push(c),
                }
            }
            output.push('"');
        }
        Value::Array(items) => {
            output.push('[');
            for (index, item) in items.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                encode(item, output);
            }
            output.push(']');
        }
        Value::Object(fields) => {
            output.push('{');
            for (index, (key, value)) in fields.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                encode(&Value::String(key.clone()), output);
                output.push(':');
                encode(value, output);
            }
            output.push('}');
        }
    }
}
