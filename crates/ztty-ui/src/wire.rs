//! Typed JSON cursor: keys/numbers/unescaped strings borrow the bounded input.
//! Unicode escape handling adapts ztty's own src/json.rs; no generic value DOM.
use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::{
    Error, Kind, MAX_ACTION, MAX_EXTENT, MAX_FRAME, MAX_NODES, MAX_STRING, Node, TextStyle,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Replace,
    Update,
    Clear,
    Capabilities,
}

pub(crate) struct Command<'a> {
    pub op: Op,
    pub surface: Option<u64>,
    pub revision: Option<u64>,
    pub base_revision: Option<u64>,
    pub root: Option<u32>,
    pub events: Option<bool>,
    pub entries: Vec<Fields<'a>>,
}

pub(crate) fn parse(bytes: &[u8]) -> Result<Command<'_>, Error> {
    if bytes.len() > MAX_FRAME {
        return Err(Error::FrameLimit);
    }
    core::str::from_utf8(bytes).map_err(|_| Error::Utf8)?;
    let mut p = Cursor {
        bytes,
        position: 0,
        references: 0,
    };
    p.expect(b'{')?;
    let mut first = true;
    let mut seen = 0u16;
    let mut version = None;
    let mut op = None;
    let mut result = Command {
        op: Op::Capabilities,
        surface: None,
        revision: None,
        base_revision: None,
        root: None,
        events: None,
        entries: Vec::new(),
    };
    while let Some(key) = p.field(&mut first)? {
        let bit = match key.as_ref() {
            "v" => 1,
            "op" => 2,
            "surface" => 4,
            "rev" => 8,
            "base_rev" => 16,
            "root" => 32,
            "events" => 64,
            "nodes" => 128,
            "set" => 256,
            _ => return Err(Error::UnknownField),
        };
        if seen & bit != 0 {
            return Err(Error::DuplicateField);
        }
        seen |= bit;
        match key.as_ref() {
            "v" => version = Some(p.number()?),
            "op" => op = Some(p.string(16)?),
            "surface" => result.surface = Some(p.number()?),
            "rev" => result.revision = Some(p.number()?),
            "base_rev" => result.base_revision = Some(p.number()?),
            "root" => result.root = Some(p.u32()?),
            "events" => result.events = Some(p.boolean()?),
            "nodes" | "set" => {
                if seen & 128 != 0 && seen & 256 != 0 {
                    return Err(Error::UnknownField);
                }
                p.expect(b'[')?;
                let mut first = true;
                while p.element(&mut first)? {
                    if result.entries.len() == MAX_NODES {
                        return Err(Error::Limit);
                    }
                    result.entries.push(Fields::read(&mut p)?);
                }
            }
            _ => unreachable!(),
        }
    }
    p.whitespace();
    if p.position != bytes.len() {
        return Err(Error::Syntax);
    }
    if version != Some(1) {
        return Err(Error::Version);
    }
    result.op = match op.as_deref() {
        Some("replace") => Op::Replace,
        Some("update") => Op::Update,
        Some("clear") => Op::Clear,
        Some("capabilities") => Op::Capabilities,
        _ => return Err(Error::Value),
    };
    let (required, allowed) = match result.op {
        Op::Replace => (1 | 2 | 4 | 8 | 32 | 128, 1 | 2 | 4 | 8 | 32 | 64 | 128),
        Op::Update => (1 | 2 | 4 | 8 | 16 | 256, 1 | 2 | 4 | 8 | 16 | 256),
        Op::Clear => (1 | 2 | 4 | 8 | 16, 1 | 2 | 4 | 8 | 16),
        Op::Capabilities => (1 | 2, 1 | 2),
    };
    if seen & !allowed != 0 {
        return Err(Error::UnknownField);
    }
    if seen & required != required {
        return Err(Error::Value);
    }
    if result.op == Op::Update && result.entries.is_empty() {
        return Err(Error::Value);
    }
    Ok(result)
}

#[derive(Default)]
pub(crate) struct Fields<'a> {
    pub id: Option<u32>,
    kind: Option<Cow<'a, str>>,
    children: Option<Vec<u32>>,
    text: Option<Cow<'a, str>>,
    style: Option<Cow<'a, str>>,
    value: Option<u64>,
    label: Option<Cow<'a, str>>,
    action: Option<Cow<'a, str>>,
    enabled: Option<bool>,
    image_id: Option<u32>,
    generation: Option<u64>,
    alt: Option<Cow<'a, str>>,
    cols: Option<u16>,
    rows: Option<u16>,
    seen: u16,
}

impl<'a> Fields<'a> {
    fn read(p: &mut Cursor<'a>) -> Result<Self, Error> {
        p.expect(b'{')?;
        let mut first = true;
        let mut f = Self::default();
        while let Some(key) = p.field(&mut first)? {
            let bit = match key.as_ref() {
                "id" => 1,
                "type" => 2,
                "children" => 4,
                "text" => 8,
                "style" => 16,
                "value" => 32,
                "label" => 64,
                "action" => 128,
                "enabled" => 256,
                "image_id" => 512,
                "generation" => 1024,
                "alt" => 2048,
                "cols" => 4096,
                "rows" => 8192,
                _ => return Err(Error::UnknownField),
            };
            if f.seen & bit != 0 {
                return Err(Error::DuplicateField);
            }
            f.seen |= bit;
            match key.as_ref() {
                "id" => f.id = Some(p.u32()?),
                "type" => f.kind = Some(p.string(16)?),
                "children" => {
                    p.expect(b'[')?;
                    let mut first = true;
                    let mut children = Vec::new();
                    while p.element(&mut first)? {
                        if children.len() == MAX_NODES {
                            return Err(Error::Limit);
                        }
                        if p.references == MAX_NODES - 1 {
                            return Err(Error::Limit);
                        }
                        p.references += 1;
                        children.push(p.u32()?);
                    }
                    f.children = Some(children);
                }
                "text" => f.text = Some(p.string(MAX_STRING)?),
                "style" => f.style = Some(p.string(16)?),
                "value" => f.value = Some(p.number()?),
                "label" => f.label = Some(p.string(MAX_STRING)?),
                "action" => f.action = Some(p.string(MAX_ACTION)?),
                "enabled" => f.enabled = Some(p.boolean()?),
                "image_id" => f.image_id = Some(p.u32()?),
                "generation" => f.generation = Some(p.number()?),
                "alt" => f.alt = Some(p.string(MAX_STRING)?),
                "cols" => f.cols = Some(p.extent()?),
                "rows" => f.rows = Some(p.extent()?),
                _ => unreachable!(),
            }
        }
        Ok(f)
    }

    pub fn into_node(self, base: Option<&Node>) -> Result<Node, Error> {
        let id = self.id.filter(|id| *id != 0).ok_or(Error::Value)?;
        if base.is_some() && self.kind.is_some() {
            return Err(Error::UnknownField);
        }
        let base = base.map(|node| &node.kind);
        let kind = self
            .kind
            .as_deref()
            .or_else(|| {
                base.map(|kind| match kind {
                    Kind::Column { .. } => "column",
                    Kind::Row { .. } => "row",
                    Kind::Text { .. } => "text",
                    Kind::Progress { .. } => "progress",
                    Kind::Button { .. } => "button",
                    Kind::Image { .. } => "image",
                })
            })
            .ok_or(Error::Value)?;
        let allowed = 1
            | 2
            | match kind {
                "column" | "row" => 4,
                "text" => 8 | 16,
                "progress" => 32 | 64,
                "button" => 64 | 128 | 256,
                "image" => 512 | 1024 | 2048 | 4096 | 8192,
                _ => return Err(Error::Value),
            };
        if self.seen & !allowed != 0 {
            return Err(Error::UnknownField);
        }
        if base.is_some() && self.seen == 1 {
            return Err(Error::Value);
        }
        let kind = match kind {
            "column" | "row" => {
                let children = self
                    .children
                    .or_else(|| match base {
                        Some(Kind::Column { children } | Kind::Row { children }) => {
                            Some(children.clone())
                        }
                        _ => None,
                    })
                    .ok_or(Error::Value)?;
                if kind == "column" {
                    Kind::Column { children }
                } else {
                    Kind::Row { children }
                }
            }
            "text" => {
                let (old, old_style) = match base {
                    Some(Kind::Text { text, style }) => (Some(text.as_str()), *style),
                    _ => (None, TextStyle::Body),
                };
                let text = text(self.text, old)?;
                let style = match self.style.as_deref() {
                    None => old_style,
                    Some("body") => TextStyle::Body,
                    Some("heading") => TextStyle::Heading,
                    Some("muted") => TextStyle::Muted,
                    Some("code") => TextStyle::Code,
                    _ => return Err(Error::Value),
                };
                Kind::Text { text, style }
            }
            "progress" => {
                let (old_value, old_label) = match base {
                    Some(Kind::Progress { value, label }) => {
                        (Some(u64::from(*value)), label.as_str())
                    }
                    _ => (None, ""),
                };
                let value = self
                    .value
                    .or(old_value)
                    .filter(|value| *value <= 10_000)
                    .ok_or(Error::Value)? as u16;
                Kind::Progress {
                    value,
                    label: text(self.label, Some(old_label))?,
                }
            }
            "button" => {
                let (old_label, old_action, old_enabled) = match base {
                    Some(Kind::Button {
                        label,
                        action,
                        enabled,
                    }) => (Some(label.as_str()), Some(action.as_str()), *enabled),
                    _ => (None, None, true),
                };
                let label = text(self.label, old_label)?;
                let action = text(self.action, old_action)?;
                if action.is_empty()
                    || action.len() > MAX_ACTION
                    || !action
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
                {
                    return Err(Error::Value);
                }
                Kind::Button {
                    label,
                    action,
                    enabled: self.enabled.unwrap_or(old_enabled),
                }
            }
            "image" => {
                let (old_id, old_generation, old_alt, old_cols, old_rows) = match base {
                    Some(Kind::Image {
                        image_id,
                        generation,
                        alt,
                        cols,
                        rows,
                    }) => (Some(*image_id), *generation, alt.as_str(), *cols, *rows),
                    _ => (None, None, "", 16, 8),
                };
                let image_id = self
                    .image_id
                    .or(old_id)
                    .filter(|id| *id != 0)
                    .ok_or(Error::Value)?;
                Kind::Image {
                    image_id,
                    generation: self.generation.or(old_generation),
                    alt: text(self.alt, Some(old_alt))?,
                    cols: self.cols.unwrap_or(old_cols),
                    rows: self.rows.unwrap_or(old_rows),
                }
            }
            _ => unreachable!(),
        };
        Ok(Node { id, kind })
    }
}

fn text(value: Option<Cow<'_, str>>, fallback: Option<&str>) -> Result<String, Error> {
    let value = match value {
        Some(value) => value.into_owned(),
        None => fallback.ok_or(Error::Value)?.to_string(),
    };
    if value.len() > MAX_STRING {
        return Err(Error::Limit);
    }
    if value
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(Error::Value);
    }
    Ok(value)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
    references: usize,
}

impl<'a> Cursor<'a> {
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
    fn expect(&mut self, byte: u8) -> Result<(), Error> {
        self.whitespace();
        if self.take(byte) {
            Ok(())
        } else {
            Err(Error::Syntax)
        }
    }
    fn field(&mut self, first: &mut bool) -> Result<Option<Cow<'a, str>>, Error> {
        self.whitespace();
        if self.take(b'}') {
            return Ok(None);
        }
        if !core::mem::replace(first, false) {
            self.expect(b',')?;
        }
        let key = self.string(32)?;
        self.expect(b':')?;
        Ok(Some(key))
    }
    fn element(&mut self, first: &mut bool) -> Result<bool, Error> {
        self.whitespace();
        if self.take(b']') {
            return Ok(false);
        }
        if !core::mem::replace(first, false) {
            self.expect(b',')?;
        }
        Ok(true)
    }
    fn boolean(&mut self) -> Result<bool, Error> {
        self.whitespace();
        for (token, value) in [(b"true".as_slice(), true), (b"false".as_slice(), false)] {
            if self.bytes.get(self.position..self.position + token.len()) == Some(token) {
                self.position += token.len();
                return Ok(value);
            }
        }
        Err(Error::Syntax)
    }
    fn number(&mut self) -> Result<u64, Error> {
        self.whitespace();
        let start = self.position;
        if self.take(b'0') {
            return Ok(0);
        }
        let mut value = 0u64;
        while let Some(b @ b'0'..=b'9') = self.bytes.get(self.position) {
            value = value
                .checked_mul(10)
                .and_then(|v| v.checked_add(u64::from(*b - b'0')))
                .ok_or(Error::Value)?;
            self.position += 1;
        }
        if self.position == start {
            return Err(Error::Syntax);
        }
        Ok(value)
    }
    fn u32(&mut self) -> Result<u32, Error> {
        self.number()?.try_into().map_err(|_| Error::Value)
    }
    fn extent(&mut self) -> Result<u16, Error> {
        let value = self.number()?;
        if value == 0 || value > u64::from(MAX_EXTENT) {
            return Err(Error::Value);
        }
        Ok(value as u16)
    }
    fn hex4(&mut self) -> Result<u32, Error> {
        let mut value = 0;
        for _ in 0..4 {
            let byte = *self.bytes.get(self.position).ok_or(Error::Syntax)?;
            self.position += 1;
            value = value * 16
                + match byte {
                    b'0'..=b'9' => u32::from(byte - b'0'),
                    b'a'..=b'f' => u32::from(byte - b'a' + 10),
                    b'A'..=b'F' => u32::from(byte - b'A' + 10),
                    _ => return Err(Error::Syntax),
                };
        }
        Ok(value)
    }
    fn string(&mut self, limit: usize) -> Result<Cow<'a, str>, Error> {
        self.expect(b'"')?;
        let start = self.position;
        let mut output: Option<String> = None;
        loop {
            let segment = self.position;
            while self
                .bytes
                .get(self.position)
                .is_some_and(|b| *b >= 0x20 && *b != b'"' && *b != b'\\')
            {
                self.position += 1;
            }
            let text = core::str::from_utf8(&self.bytes[segment..self.position])
                .map_err(|_| Error::Utf8)?;
            if let Some(output) = output.as_mut() {
                if output.len() + text.len() > limit {
                    return Err(Error::Limit);
                }
                output.push_str(text);
            } else if self.position - start > limit {
                return Err(Error::Limit);
            }
            match self.bytes.get(self.position).copied() {
                Some(b'"') => {
                    self.position += 1;
                    return Ok(match output {
                        Some(text) => Cow::Owned(text),
                        None => Cow::Borrowed(text),
                    });
                }
                Some(b'\\') => {
                    self.position += 1;
                    let escape = *self.bytes.get(self.position).ok_or(Error::Syntax)?;
                    self.position += 1;
                    let c = match escape {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let high = self.hex4()?;
                            let scalar = if (0xd800..=0xdbff).contains(&high) {
                                if !self.take(b'\\') || !self.take(b'u') {
                                    return Err(Error::Syntax);
                                }
                                let low = self.hex4()?;
                                if !(0xdc00..=0xdfff).contains(&low) {
                                    return Err(Error::Syntax);
                                }
                                0x10000 + ((high - 0xd800) << 10) + low - 0xdc00
                            } else {
                                high
                            };
                            char::from_u32(scalar).ok_or(Error::Syntax)?
                        }
                        _ => return Err(Error::Syntax),
                    };
                    let output = output.get_or_insert_with(|| text.to_string());
                    if output.len() + c.len_utf8() > limit {
                        return Err(Error::Limit);
                    }
                    output.push(c);
                }
                _ => return Err(Error::Syntax),
            }
        }
    }
}
