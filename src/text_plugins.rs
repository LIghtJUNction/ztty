//! Bounded local registrations. Registration does not load executable code.
use alloc::{format, string::String, vec::Vec};

pub const MAX_PLUGINS: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Registration {
    pub name: String,
    pub path: String,
}

impl Registration {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        if value.chars().any(char::is_control) {
            return Err("plugin registration cannot contain control characters");
        }
        let (name, path) = value
            .split_once('=')
            .ok_or("text-plugin needs NAME=/absolute/path")?;
        let name = name.trim();
        let path = path.trim();
        if name.is_empty()
            || name.len() > 32
            || !name.as_bytes()[0].is_ascii_alphabetic()
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
        {
            return Err(
                "plugin name must be 1..32 ASCII letters, digits, - or _, starting with a letter",
            );
        }
        if !path.starts_with('/') || path.len() > 4096 || path.chars().any(char::is_control) {
            return Err("plugin path must be an absolute UTF-8 path of at most 4096 bytes");
        }
        Ok(Self {
            name: name.to_ascii_lowercase(),
            path: path.into(),
        })
    }

    pub fn config(&self) -> String {
        format!("{}={}", self.name, self.path)
    }
}

/// Later config entries and CLI registrations replace the same normalized name.
pub fn set(entries: &mut Vec<Registration>, value: &str) -> Result<(), &'static str> {
    let entry = Registration::parse(value)?;
    if let Some(existing) = entries.iter_mut().find(|item| item.name == entry.name) {
        *existing = entry;
    } else if entries.len() < MAX_PLUGINS {
        entries.push(entry);
    } else {
        return Err("at most 8 text plugins can be registered");
    }
    Ok(())
}
