//! Windowless Linux clients of the existing per-user session broker.
use crate::{
    broker,
    json::{self, Value, object},
    os,
    session_identity::ExpectedSession,
};
use alloc::{string::String, vec::Vec};
use core::ffi::CStr;

const HELP: &str = "\
ztty send SESSION [--expect INFO_JSON] [--enter] (--stdin | -- DATA)
ztty read SESSION [--expect INFO_JSON] [--cursor BYTE] [--max-bytes N]
ztty wait SESSION [--expect INFO_JSON] [--until output|eof] [--cursor BYTE] [--max-bytes N] [--timeout-ms MS]
ztty exec --new NAME [--max-bytes N] [--timeout-ms MS] -- PROGRAM [ARG...]
ztty exec SESSION -- PROGRAM [ARG...]  (unsupported for interactive sessions)

send needs released input control. --enter appends one CR. --stdin preserves binary input.
read and wait return JSON. Defaults: cursor 0, 16384 bytes, wait for output, 5000 ms.
Limits: 65536 input/page bytes and 30000 ms. Use info or a prior page with --expect.
exec starts a named, headless command PTY. Completion requires PTY EOF and child status.
Timeout leaves the job running. Completed jobs with unread pages remain available.
Exit: 0 success, 1 error, 3 EOF on output wait, 4 session changed/gone, 69 unsupported,
124 timeout. Completed exec returns the child status; inspect JSON status to distinguish.
";

#[derive(Default)]
struct Options<'a> {
    expected: Option<Value>,
    cursor: u64,
    max_bytes: Option<u64>,
    timeout_ms: Option<u64>,
    until_eof: bool,
    enter: bool,
    stdin: bool,
    payload: &'a [&'a CStr],
}

fn utf8(value: &CStr) -> Result<&str, String> {
    value
        .to_str()
        .map_err(|_| "option or selector must be UTF-8".into())
}

fn options<'a>(op: &str, mut args: &'a [&'a CStr]) -> Result<Options<'a>, String> {
    let mut opts = Options::default();
    while let Some((arg, rest)) = args.split_first() {
        args = rest;
        let flag = utf8(arg)?;
        match flag {
            "--" if matches!(op, "send" | "exec") => {
                opts.payload = rest;
                break;
            }
            "--enter" if op == "send" => opts.enter = true,
            "--stdin" if op == "send" => opts.stdin = true,
            _ => {
                let (value, rest) = args
                    .split_first()
                    .ok_or("missing option value; see --help")?;
                args = rest;
                let number = || {
                    utf8(value)?
                        .parse::<u64>()
                        .map_err(|_| String::from("expected an unsigned integer"))
                };
                match flag {
                    "--expect" if op != "exec" => {
                        opts.expected = Some(json::parse(value.to_bytes())?);
                    }
                    "--cursor" if matches!(op, "read" | "wait") => opts.cursor = number()?,
                    "--max-bytes" if op != "send" => {
                        opts.max_bytes = Some(number()?);
                        if !matches!(opts.max_bytes, Some(1..=65536)) {
                            return Err("--max-bytes must be 1..65536".into());
                        }
                    }
                    "--timeout-ms" if matches!(op, "exec" | "wait") => {
                        opts.timeout_ms = Some(number()?);
                        if opts.timeout_ms > Some(30000) {
                            return Err("--timeout-ms must be 0..30000".into());
                        }
                    }
                    "--until" if op == "wait" => {
                        opts.until_eof = match utf8(value)? {
                            "output" => false,
                            "eof" => true,
                            _ => return Err("--until must be output or eof".into()),
                        }
                    }
                    _ => return Err("invalid option; see --help".into()),
                }
            }
        }
    }
    if op == "send" && !matches!((opts.stdin, opts.payload.len()), (true, 0) | (false, 1)) {
        return Err("send requires --stdin or -- followed by exactly one argument".into());
    }
    if op == "exec" && opts.payload.is_empty() {
        return Err("exec requires -- PROGRAM [ARG...]".into());
    }
    Ok(opts)
}

fn identity(info: &Value) -> Result<Value, String> {
    let get = |key| info.get(key).cloned().ok_or("incomplete session identity");
    let value = object([
        ("uri", get("uri")?),
        ("id", get("id")?),
        ("created_at", get("created_at")?),
        ("broker_pid", get("broker_pid")?),
    ]);
    ExpectedSession::parse(&value)?;
    Ok(value)
}

struct Target {
    identity: Value,
    epoch: u64,
}

impl Target {
    fn from_info(info: &Value) -> Result<Self, String> {
        Ok(Self {
            identity: identity(info)?,
            epoch: info
                .get("control_epoch")
                .and_then(Value::as_u64)
                .ok_or("unsupported broker: headless CLI requires control_epoch")?,
        })
    }

    fn request(&self, op: &str) -> Value {
        object([
            ("op", op.into()),
            ("session", self.identity.get("uri").unwrap().clone()),
            ("expected_session", self.identity.clone()),
        ])
    }

    fn page(
        &self,
        opts: &Options,
        until: broker::ReadUntil,
        wait_ms: u64,
    ) -> Result<Value, String> {
        let mut read = self.request("read");
        read.set("cursor", opts.cursor.into());
        read.set("max_bytes", opts.max_bytes.unwrap_or(16384).into());
        read.set("wait_ms", wait_ms.into());
        read.set("_until", until.name().into());
        let page = broker::request(&read)?;
        // A parked read is bound to the session ID, but its URI may have been renamed.
        if identity(&page)? != self.identity {
            return Err("session identity changed".into());
        }
        Ok(page)
    }
}

fn emit(mut value: Value, status: &str, code: i32) -> i32 {
    value.set("status", status.into());
    os::print(1, json::stringify(&value).as_bytes());
    os::print(1, b"\n");
    code
}

fn send(target: &Target, opts: &Options) -> Result<i32, String> {
    let mut data = Vec::new();
    if opts.stdin {
        let mut buffer = [0u8; 8192];
        loop {
            let count = unsafe { os::read(0, buffer.as_mut_ptr().cast(), buffer.len()) };
            if count == 0 {
                break;
            }
            if count < 0 {
                if os::errno() == os::EINTR {
                    continue;
                }
                return Err("could not read standard input".into());
            }
            data.extend_from_slice(&buffer[..count as usize]);
            if data.len() > 65536 {
                return Err("input exceeds 65536 bytes".into());
            }
        }
    } else {
        data.extend_from_slice(opts.payload[0].to_bytes());
    }
    if opts.enter {
        data.push(b'\r');
    }
    if data.len() > 65536 {
        return Err("input exceeds 65536 bytes".into());
    }
    let mut write = target.request("cli-send");
    write.set("control_epoch", target.epoch.into());
    write.set("_data_hex", broker::hex(&data).into());
    let result = broker::request(&write).map_err(|error| {
        if error.starts_with("session broker")
            || error.contains("drain timed out")
            || error.contains("while draining")
        {
            alloc::format!(
                "{error}; input may have reached the PTY, inspect the session before sending again"
            )
        } else {
            error
        }
    })?;
    Ok(emit(result, "sent", 0))
}

fn flag(page: &Value, key: &str) -> bool {
    page.get(key).and_then(Value::as_bool) == Some(true)
}

fn exec(name: &str, opts: &Options) -> Result<i32, String> {
    let create = object([
        ("op", "create".into()),
        ("name", name.into()),
        ("origin", "job".into()),
        ("headless", true.into()),
        (
            "_argv",
            Value::Array(
                opts.payload
                    .iter()
                    .map(|arg| broker::hex(arg.to_bytes()).into())
                    .collect(),
            ),
        ),
    ]);
    let target = Target::from_info(&broker::request(&create)?)?;
    let mut page = target.page(
        opts,
        broker::ReadUntil::Exit,
        opts.timeout_ms.unwrap_or(5000),
    )?;
    let code = page.get("exit_code").and_then(Value::as_i64);
    if !flag(&page, "output_eof") || code.is_none() {
        page.set("retained", true.into());
        return Ok(emit(page, "timeout", 124));
    }
    // Keep unread pages for the caller. On timeout or loss of confirmation the
    // caller can inspect the existing named job. Never re-create or signal it.
    page.set("retained", true.into());
    if !flag(&page, "has_more") {
        let mut finish = target.request("job-finish");
        finish.set("control_epoch", target.epoch.into());
        match broker::request(&finish) {
            Ok(retained) => page.set("retained", retained),
            Err(reason) => {
                page.set("retained", Value::Null);
                page.set("cleanup_error", reason.into());
            }
        }
    }
    Ok(emit(
        page,
        "completed",
        code.ok_or("missing job exit status")? as i32,
    ))
}

fn command(args: &[&CStr]) -> Result<i32, String> {
    let op = utf8(args[0])?;
    if matches!(args.get(1), Some(arg) if *arg == c"--help" || *arg == c"-h") {
        os::print(1, HELP.as_bytes());
        return Ok(0);
    }
    let mut rest = &args[1..];
    let new_job = op == "exec" && rest.first() == Some(&c"--new");
    if new_job {
        rest = &rest[1..];
    }
    let (name, rest) = rest
        .split_first()
        .ok_or("session selector required; see --help")?;
    let name = utf8(name)?;
    let opts = options(op, rest)?;
    if op == "exec" {
        if new_job {
            return exec(name, &opts);
        }
        return Err("unsupported session exec: interactive shells do not supply a correlated job receipt; use exec --new".into());
    }
    let mut info = object([("op", "info".into()), ("session", name.into())]);
    if let Some(expected) = &opts.expected {
        info.set("expected_session", identity(expected)?);
    }
    let target = Target::from_info(&broker::request(&info)?)?;
    if op == "send" {
        if let Some(epoch) = opts.expected.as_ref().and_then(|v| v.get("control_epoch"))
            && epoch.as_u64() != Some(target.epoch)
        {
            return Err("input control epoch changed".into());
        }
        return send(&target, &opts);
    }
    if op == "read" {
        return Ok(emit(
            target.page(&opts, broker::ReadUntil::Output, 0)?,
            "read",
            0,
        ));
    }
    if opts.until_eof {
        let page = target.page(
            &opts,
            broker::ReadUntil::Eof,
            opts.timeout_ms.unwrap_or(5000),
        )?;
        return Ok(if flag(&page, "output_eof") {
            emit(page, "eof", 0)
        } else {
            emit(page, "timeout", 124)
        });
    }
    let page = target.page(
        &opts,
        broker::ReadUntil::Output,
        opts.timeout_ms.unwrap_or(5000),
    )?;
    let (status, code) = if flag(&page, "truncated") {
        ("truncated", 0)
    } else if page
        .get("output_base64")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty())
    {
        ("output", 0)
    } else if flag(&page, "output_eof") {
        ("eof", 3)
    } else {
        ("timeout", 124)
    };
    Ok(emit(page, status, code))
}

pub fn run(args: &[&CStr]) -> Result<i32, String> {
    match command(args) {
        Err(reason)
            if reason.starts_with("session identity changed")
                || reason.starts_with("session not found:")
                || reason == "session closed while waiting for output" =>
        {
            Ok(emit(
                object([("detail", reason.into())]),
                "session_changed_or_gone",
                4,
            ))
        }
        Err(reason) if reason.starts_with("unsupported ") || reason == "invalid session origin" => {
            Ok(emit(object([("detail", reason.into())]), "unsupported", 69))
        }
        result => result,
    }
}
