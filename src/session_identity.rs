//! Borrowed identity guard for targeting a particular terminal session.
//! These public identifiers prevent accidental reuse; they are not credentials.
use crate::json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExpectedSession<'a> {
    uri: &'a str,
    id: u64,
    created_at: u64,
    broker_pid: u32,
}

impl<'a> ExpectedSession<'a> {
    pub fn parse(value: &'a Value) -> Result<Self, &'static str> {
        let Value::Object(fields) = value else {
            return Err("expected_session must be an object");
        };
        const KEYS: [&str; 4] = ["uri", "id", "created_at", "broker_pid"];
        if fields.len() != KEYS.len()
            || fields.iter().enumerate().any(|(index, (key, _))| {
                !KEYS.contains(&key.as_str())
                    || fields[..index].iter().any(|(previous, _)| previous == key)
            })
        {
            return Err("expected_session requires exactly uri, id, created_at and broker_pid");
        }
        let uri = value
            .get("uri")
            .and_then(Value::as_str)
            .ok_or("expected_session.uri must be a canonical ztty URI")?;
        // Match workspace snapshot identity syntax. An expected URI is always
        // canonical, even when the outer selector uses an unambiguous short name.
        let (origin, name) = uri
            .strip_prefix("ztty://")
            .and_then(|rest| rest.split_once('/'))
            .ok_or("expected_session.uri must be a canonical ztty URI")?;
        if uri.len() > 2048
            || origin.is_empty()
            || name.is_empty()
            || uri.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return Err("expected_session.uri must be a canonical ztty URI");
        }
        let id = value
            .get("id")
            .and_then(Value::as_u64)
            .filter(|id| *id != 0)
            .ok_or("expected_session.id must be a positive integer")?;
        let created_at = value
            .get("created_at")
            .and_then(Value::as_u64)
            .filter(|created_at| *created_at != 0)
            .ok_or("expected_session.created_at must be a positive integer")?;
        let broker_pid = value
            .get("broker_pid")
            .and_then(Value::as_u64)
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|pid| *pid != 0)
            .ok_or("expected_session.broker_pid must be a positive 32-bit integer")?;
        Ok(Self {
            uri,
            id,
            created_at,
            broker_pid,
        })
    }

    pub fn matches(self, uri: &str, id: u64, created_at: u64, broker_pid: u32) -> bool {
        self.uri == uri
            && self.id == id
            && self.created_at == created_at
            && self.broker_pid == broker_pid
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::{self, object};

    fn identity() -> Value {
        object([
            ("uri", "ztty://mcp/stdio/work".into()),
            ("id", 7u64.into()),
            ("created_at", 123456u64.into()),
            ("broker_pid", 42u32.into()),
        ])
    }

    #[test]
    fn all_four_fields_bind_the_expected_stream() {
        let value = identity();
        let expected = ExpectedSession::parse(&value).unwrap();
        assert!(expected.matches("ztty://mcp/stdio/work", 7, 123456, 42));
        assert!(!expected.matches("ztty://mcp/stdio/other", 7, 123456, 42));
        assert!(!expected.matches("ztty://mcp/stdio/work", 8, 123456, 42));
        assert!(!expected.matches("ztty://mcp/stdio/work", 7, 123457, 42));
        assert!(!expected.matches("ztty://mcp/stdio/work", 7, 123456, 43));
    }

    #[test]
    fn identity_fields_are_complete_unique_and_strictly_typed() {
        assert!(ExpectedSession::parse(&Value::Null).is_err());
        let Value::Object(fields) = identity() else {
            unreachable!()
        };
        for index in 0..fields.len() {
            let mut missing = fields.clone();
            missing.remove(index);
            assert!(ExpectedSession::parse(&Value::Object(missing)).is_err());
            let mut duplicate = fields.clone();
            duplicate[index] = fields[(index + 1) % fields.len()].clone();
            assert!(ExpectedSession::parse(&Value::Object(duplicate)).is_err());
            let mut invalid = identity();
            invalid.set(&fields[index].0, Value::Null);
            assert!(ExpectedSession::parse(&invalid).is_err());
        }
        let mut extra = identity();
        extra.set("cursor", 0u64.into());
        assert!(ExpectedSession::parse(&extra).is_err());
        for (key, invalid) in [
            ("id", 0u64.into()),
            ("id", "7".into()),
            ("id", (-1i32).into()),
            ("created_at", 0u64.into()),
            ("created_at", true.into()),
            ("created_at", (-1i32).into()),
            ("broker_pid", 0u64.into()),
            ("broker_pid", (u32::MAX as u64 + 1).into()),
        ] {
            let mut value = identity();
            value.set(key, invalid);
            assert!(ExpectedSession::parse(&value).is_err(), "{key}");
        }
        for uri in [
            "work",
            "ztty://",
            "ztty:///work",
            "ztty://local/",
            "ztty://local/a b",
        ] {
            let mut value = identity();
            value.set("uri", uri.into());
            assert!(ExpectedSession::parse(&value).is_err(), "{uri}");
        }
    }

    #[test]
    fn large_identifiers_are_compared_without_number_rounding() {
        let value = json::parse(
            br#"{"uri":"ztty://local/large","id":9007199254740993,"created_at":18446744073709551615,"broker_pid":4294967295}"#,
        )
        .unwrap();
        let expected = ExpectedSession::parse(&value).unwrap();
        assert!(expected.matches("ztty://local/large", 9007199254740993, u64::MAX, u32::MAX));
        assert!(!expected.matches("ztty://local/large", 9007199254740992, u64::MAX, u32::MAX));
    }
}
