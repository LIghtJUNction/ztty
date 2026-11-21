//! Linux shared-session frontend. The broker owns PTYs and native windows.
use crate::{Config, appearance::Appearance, backend::Preference, broker, json, os};
use alloc::string::String;
use core::ffi::CStr;

pub fn run(cfg: Config) -> Result<i32, String> {
    use json::{Value, object};
    if cfg.target.is_none() {
        broker::select_gui_broker()?;
    }
    let mut request = object([
        ("op", Value::from("create")),
        ("origin", Value::from("local")),
        ("headless", Value::Bool(false)),
        ("_backend", Value::from(cfg.backend.legacy_backend().name())),
        ("observe", Value::Bool(cfg.observe)),
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
    if cfg.backend == Preference::Auto {
        request.set("_backend_auto", Value::Bool(true));
    }
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
    if let Some(profile) = cfg.ssh_profile {
        request.set("_ssh", profile);
    }
    if let Some(count) = cfg.new_windows {
        request.set("op", "new-windows".into());
        request.set("count", count.into());
        let result = broker::request(&request)?;
        os::print(1, json::stringify(&result).as_bytes());
        os::print(1, b"\n");
        return Ok(i32::from(result.get("error").is_some()));
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
    let mut observe = false;
    let mut backend = Preference::Auto;
    let mut args = args;
    while let Some(first) = args.first() {
        match first.to_bytes() {
            b"--observe" => {
                observe = true;
                args = &args[1..];
            }
            b"--backend" => {
                backend = Preference::parse(
                    args.get(1)
                        .ok_or("--backend needs auto, x11 or wayland")?
                        .to_bytes(),
                )?;
                args = &args[2..];
            }
            _ => break,
        }
    }
    if args.len() != 1 {
        return Err(
            "usage: ztty attach [--observe] [--backend auto|x11|wayland] [NAME | ztty://ADDRESS]"
                .into(),
        );
    }
    let name = args[0]
        .to_str()
        .map_err(|_| "session selector must be UTF-8")?;
    let appearance = Appearance::load(None)?;
    let mut request = object([
        ("op", Value::from("attach")),
        ("session", Value::from(name)),
        ("observe", Value::Bool(observe)),
        ("_backend", Value::from(backend.legacy_backend().name())),
        ("_appearance", appearance.to_json()),
    ]);
    if backend == Preference::Auto {
        // Old brokers ignore `_backend_auto`. Supply the existing concrete
        // backend as well, so attaching their native view remains compatible.
        let current = broker::request(&object([
            ("op", Value::from("info")),
            ("session", Value::from(name)),
        ]))?;
        if current.get("attached").and_then(Value::as_bool) == Some(true)
            && let Some(existing @ ("x11" | "wayland")) =
                current.get("backend").and_then(Value::as_str)
        {
            request.set("_backend", Value::from(existing));
        }
        request.set("_backend_auto", Value::Bool(true));
    }
    let value = broker::request(&request)?;
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
    if op == "restore-layout" {
        if args.len() != 2 {
            return Err("usage: ztty restore-layout FILE".into());
        }
        let bytes = os::read_config(args[1])
            .map_err(|e| alloc::format!("cannot read layout (OS error {e})"))?;
        let layout = json::parse(&bytes).map_err(String::from)?;
        request.set("layout", layout);
    } else if op == "list" {
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
            "info" | "detach" | "close" | "commands" | "save-layout" if args.len() == 2 => {}
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
