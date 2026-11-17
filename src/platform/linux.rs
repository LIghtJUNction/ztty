//! Linux shared-session frontend. The broker owns PTYs and X11 windows.
use crate::{Config, appearance::Appearance, broker, json, os};
use alloc::string::String;
use core::ffi::CStr;

pub fn run(cfg: Config) -> Result<i32, String> {
    use json::{Value, object};
    let mut request = object([
        ("op", Value::from("create")),
        ("origin", Value::from("local")),
        ("headless", Value::Bool(false)),
        ("cols", Value::from(cfg.cols)),
        ("rows", Value::from(cfg.rows)),
        ("history", Value::from(cfg.history)),
        ("_appearance", cfg.appearance.to_json()),
        (
            "_argv",
            Value::Array(
                cfg.command
                    .iter()
                    .map(|s| Value::from(broker::hex(s.to_bytes())))
                    .collect(),
            ),
        ),
    ]);
    if let Some(name) = cfg.name {
        request.set("name", Value::from(name));
    }
    if let Some((target, mode)) = cfg.target {
        request.set("target", Value::from(target));
        request.set("layout", Value::from(mode));
    }
    if let Some(cwd) = cfg.cwd {
        request.set("cwd", Value::from(cwd));
    }
    if let Some(path) = cfg.config_path {
        request.set("_config", Value::from(broker::hex(path.to_bytes())));
    }
    let created = broker::request(&request)?;
    wait_session(&created)
}

fn wait_session(session: &json::Value) -> Result<i32, String> {
    let uri = session
        .get("uri")
        .and_then(json::Value::as_str)
        .ok_or("missing session address")?;
    let attachment = session
        .get("attachment")
        .and_then(json::Value::as_u64)
        .ok_or("missing attachment token")?;
    let value = broker::request(&json::object([
        ("op", json::Value::from("wait")),
        ("session", json::Value::from(uri)),
        ("attachment", json::Value::from(attachment)),
    ]))?;
    Ok(value
        .get("exit_code")
        .and_then(json::Value::as_i64)
        .unwrap_or(0) as i32)
}

pub fn attach(args: &[&CStr]) -> Result<i32, String> {
    use json::{Value, object};
    if args.is_empty() || args == [c"--list"] || args == [c"list"] {
        let value = broker::request(&object([("op", Value::from("list"))]))?;
        let sessions = value
            .get("sessions")
            .and_then(Value::as_array)
            .ok_or("invalid session list")?;
        if sessions.is_empty() {
            os::print(1, b"No ztty sessions.\n");
            return Ok(0);
        }
        os::print(1, b"SESSION\tSTATE\tSIZE\tWINDOW\n");
        for session in sessions {
            let line = alloc::format!(
                "{}\t{}\t{}x{}\t{}\n",
                session.get("uri").and_then(Value::as_str).unwrap_or(""),
                if session.get("running").and_then(Value::as_bool) == Some(true) {
                    "running"
                } else {
                    "exited"
                },
                session.get("cols").and_then(Value::as_u64).unwrap_or(0),
                session.get("rows").and_then(Value::as_u64).unwrap_or(0),
                if session.get("attached").and_then(Value::as_bool) == Some(true) {
                    "attached"
                } else {
                    "detached"
                }
            );
            os::print(1, line.as_bytes());
        }
        return Ok(0);
    }
    let (observe, args) = if args.first().is_some_and(|s| s.to_bytes() == b"--observe") {
        (true, &args[1..])
    } else {
        (false, args)
    };
    if args.len() != 1 {
        return Err("usage: ztty attach [--observe] [NAME | ztty://ADDRESS]".into());
    }
    let name = args[0]
        .to_str()
        .map_err(|_| "session selector must be UTF-8")?;
    let appearance = Appearance::load(None)?;
    let value = broker::request(&object([
        ("op", Value::from("attach")),
        ("session", Value::from(name)),
        ("observe", Value::Bool(observe)),
        ("_appearance", appearance.to_json()),
    ]))?;
    wait_session(&value)
}

/// Scriptable desktop/session actions return one bounded JSON result. The
/// local broker still checks input ownership on every mutating operation.
pub fn command(args: &[&CStr]) -> Result<i32, String> {
    use json::{Value, object};
    let text = |index: usize| -> Result<&str, String> {
        args.get(index)
            .ok_or_else(|| String::from("missing argument; run ztty --help"))?
            .to_str()
            .map_err(|_| String::from("argument must be UTF-8"))
    };
    let op = text(0)?;
    let mut request = object([("op", Value::from(op))]);
    if op == "list" {
        if args.len() > 2 {
            return Err("usage: ztty list [FILTER]".into());
        }
        if args.len() == 2 {
            request.set("filter", Value::from(text(1)?));
        }
    } else {
        let selector = text(1)?;
        request.set("session", Value::from(selector));
        match op {
            "info" | "detach" | "commands" if args.len() == 2 => {}
            "command-output" if args.len() == 3 => {
                request.set("op", Value::from("command-read"));
                let id = text(2)?
                    .parse::<u64>()
                    .map_err(|_| String::from("command ID must be an integer"))?;
                request.set("command_id", Value::from(id));
            }
            "rename" if args.len() == 3 => request.set("name", Value::from(text(2)?)),
            "action" if args.len() == 3 => request.set("action", Value::from(text(2)?)),
            "move" if (3..=4).contains(&args.len()) => {
                request.set("target", Value::from(text(2)?));
                request.set(
                    "layout",
                    Value::from(if args.len() == 4 { text(3)? } else { "tab" }),
                );
            }
            "control" if (3..=4).contains(&args.len()) => {
                request.set("action", Value::from(text(2)?));
                if args.len() == 4 {
                    if text(3)? != "--takeover" {
                        return Err("expected --takeover".into());
                    }
                    request.set("takeover", Value::Bool(true));
                }
            }
            _ => return Err("invalid arguments; run ztty --help".into()),
        }
    }
    let value = broker::request(&request)?;
    os::print(1, json::stringify(&value).as_bytes());
    os::print(1, b"\n");
    Ok(0)
}
