//! MCP tools over newline-delimited stdio or Streamable HTTP.
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::ffi::{CStr, c_int, c_void};

use crate::json::{self, Value, object};
use crate::os;

#[path = "mcp_http.rs"]
mod http;

pub(super) const VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];
const HELP: &str = "Usage: ztty mcp [--transport stdio|http] [--host HOST] [--port PORT] [--token TOKEN]\n\nDefaults: stdio; HTTP host 127.0.0.1, port 8732, endpoint /mcp.\nUse --port 0 to choose a free port. ZTTY_MCP_TOKEN supplies a bearer token.\nA bearer token is required for a non-loopback HTTP listener.\nMCP sessions create persistent PTYs; use ztty attach to list and view them.\n";
const TOOLS: &str = r#"[
{"name":"session_create","description":"Create a persistent terminal session in the shared ztty process. Commands are argv; use an explicit shell for shell syntax. Attach the returned URI with ztty attach.","inputSchema":{"type":"object","properties":{"name":{"type":"string","description":"Optional unique name within this MCP transport namespace"},"command":{"type":"array","items":{"type":"string"},"minItems":1,"description":"Executable and arguments; defaults to the configured shell"},"cwd":{"type":"string"},"cols":{"type":"integer","minimum":2,"maximum":512},"rows":{"type":"integer","minimum":2,"maximum":256},"history":{"type":"integer","minimum":0,"maximum":100000}},"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":false,"openWorldHint":true}},
{"name":"session_list","description":"List terminal sessions, canonical ztty:// URIs, process state and attached windows.","inputSchema":{"type":"object","properties":{},"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}},
{"name":"session_read","description":"Read terminal output from a byte cursor and the current screen. max_bytes is at most 65536; a text page may extend by up to 3 bytes to finish a UTF-8 character. Reuse next_cursor; truncated reports expired output.","inputSchema":{"type":"object","properties":{"session":{"type":"string","description":"Canonical ztty:// URI or unambiguous short name"},"cursor":{"type":"integer","minimum":0},"max_bytes":{"type":"integer","minimum":1,"maximum":65536}},"required":["session"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}},
{"name":"session_write","description":"Send up to 65536 UTF-8 bytes or terminal control bytes to a session. Include a newline to submit a shell command; use \\u0003 to interrupt.","inputSchema":{"type":"object","properties":{"session":{"type":"string"},"data":{"type":"string","maxLength":65536}},"required":["session","data"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":true,"openWorldHint":true}},
{"name":"session_resize","description":"Resize a session PTY and screen; attached windows use the new terminal dimensions.","inputSchema":{"type":"object","properties":{"session":{"type":"string"},"cols":{"type":"integer","minimum":2,"maximum":512},"rows":{"type":"integer","minimum":2,"maximum":256}},"required":["session","cols","rows"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":false,"openWorldHint":false}},
{"name":"session_close","description":"Close a terminal session, hang up its child process and release its window/history.","inputSchema":{"type":"object","properties":{"session":{"type":"string"}},"required":["session"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":true,"openWorldHint":false}}
]"#;

unsafe extern "C" {
    fn read(fd: c_int, buffer: *mut c_void, count: usize) -> isize;
}

pub fn run(args: &[&CStr]) -> Result<i32, String> {
    let mut transport = "stdio";
    let mut host = "127.0.0.1";
    let mut port = 8732u16;
    let mut token = os::env(c"ZTTY_MCP_TOKEN")
        .map(|bytes| core::str::from_utf8(bytes).map(ToString::to_string))
        .transpose()
        .map_err(|_| "ZTTY_MCP_TOKEN must be UTF-8")?;
    let mut http_option = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index]
            .to_str()
            .map_err(|_| "MCP arguments must be UTF-8")?;
        match arg {
            "--help" | "-h" => {
                os::print(1, HELP.as_bytes());
                return Ok(0);
            }
            "--stdio" => transport = "stdio",
            "--http" => transport = "http",
            "--transport" | "--host" | "--port" | "--token" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| format!("{arg} needs a value"))?
                    .to_str()
                    .map_err(|_| "MCP arguments must be UTF-8")?;
                match arg {
                    "--transport" => transport = value,
                    "--host" => {
                        host = value;
                        http_option = true;
                    }
                    "--port" => {
                        port = value.parse().map_err(|_| "--port must be 0..65535")?;
                        http_option = true;
                    }
                    "--token" => {
                        token = Some(value.into());
                        http_option = true;
                    }
                    _ => unreachable!(),
                }
            }
            _ => return Err(format!("unknown MCP argument: {arg}; run ztty mcp --help")),
        }
        index += 1;
    }
    if token.as_ref().is_some_and(|t| {
        t.is_empty() || t.len() > 4096 || t.bytes().any(|b| b <= 0x20 || b >= 0x7f)
    }) {
        return Err("MCP bearer token must be 1..4096 visible ASCII bytes".into());
    }
    match transport {
        "stdio" if http_option => Err("--host, --port and --token require --transport http".into()),
        "stdio" => stdio(),
        "http" | "streamable-http" => http::run(host, port, token.as_deref()),
        _ => Err("--transport must be stdio or http".into()),
    }
}

pub(super) struct RpcSession {
    initialized: bool,
    ready: bool,
    pub(super) version: String,
}

impl RpcSession {
    pub(super) fn new() -> Self {
        Self {
            initialized: false,
            ready: false,
            version: VERSIONS[0].into(),
        }
    }
}

pub(super) fn error(id: Value, code: i32, message: &str) -> Value {
    object([
        ("jsonrpc", "2.0".into()),
        ("id", id),
        (
            "error",
            object([("code", code.into()), ("message", message.into())]),
        ),
    ])
}

fn success(id: Value, result: Value) -> Value {
    object([("jsonrpc", "2.0".into()), ("id", id), ("result", result)])
}

pub(super) fn dispatch(message: &Value, session: &mut RpcSession, origin: &str) -> Option<Value> {
    let id = message.get("id").cloned();
    let valid_id = matches!(&id, None | Some(Value::String(_) | Value::Number(_)));
    if !matches!(message, Value::Object(_))
        || message.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || !valid_id
    {
        return Some(error(Value::Null, -32600, "Invalid Request"));
    }
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        if id.is_some() && (message.get("result").is_some() ^ message.get("error").is_some()) {
            return None;
        }
        return Some(error(Value::Null, -32600, "Invalid Request"));
    };
    if id.is_none() {
        if method == "notifications/initialized" && session.initialized {
            session.ready = true;
        }
        return None;
    }
    let id = id.unwrap_or(Value::Null);
    let empty = object([]);
    let params = message.get("params").unwrap_or(&empty);
    if !matches!(params, Value::Object(_)) {
        return Some(error(id, -32602, "params must be an object"));
    }
    if method == "initialize" {
        if session.initialized {
            return Some(error(id, -32600, "Session is already initialized"));
        }
        let Some(requested) = params.get("protocolVersion").and_then(Value::as_str) else {
            return Some(error(id, -32602, "protocolVersion is required"));
        };
        if !matches!(params.get("capabilities"), Some(Value::Object(_)))
            || params
                .get("clientInfo")
                .and_then(|v| v.get("name"))
                .and_then(Value::as_str)
                .is_none()
            || params
                .get("clientInfo")
                .and_then(|v| v.get("version"))
                .and_then(Value::as_str)
                .is_none()
        {
            return Some(error(
                id,
                -32602,
                "capabilities and clientInfo.name/version are required",
            ));
        }
        session.version = if VERSIONS.contains(&requested) {
            requested
        } else {
            VERSIONS[0]
        }
        .into();
        session.initialized = true;
        return Some(success(id, object([
            ("protocolVersion", session.version.clone().into()),
            ("capabilities", object([("tools", object([("listChanged", false.into())]))])),
            ("serverInfo", object([("name", "ztty".into()), ("version", env!("CARGO_PKG_VERSION").into())])),
            ("instructions", "Create persistent PTYs, read output incrementally, write input, and attach their ztty:// URIs in a desktop window.".into()),
        ])));
    }
    if method == "ping" {
        return Some(success(id, object([])));
    }
    if !session.initialized || !session.ready {
        return Some(error(
            id,
            -32000,
            "Initialize the session and send notifications/initialized first",
        ));
    }
    match method {
        "tools/list" => {
            if params.get("cursor").is_some() {
                return Some(error(id, -32602, "No pagination cursor is supported"));
            }
            Some(success(
                id,
                object([(
                    "tools",
                    json::parse(TOOLS.as_bytes()).expect("static tool schema"),
                )]),
            ))
        }
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(error(id, -32602, "tool name is required"));
            };
            let arguments = params.get("arguments").unwrap_or(&empty);
            let request = match tool_request(name, arguments, origin) {
                Ok(request) => request,
                Err(message) => return Some(error(id, -32602, message)),
            };
            let (result, failed) = match crate::broker::request(&request) {
                Ok(value) => (value, false),
                Err(message) => (object([("error", message.into())]), true),
            };
            Some(success(
                id,
                object([
                    (
                        "content",
                        Value::Array(alloc::vec![object([
                            ("type", "text".into()),
                            ("text", json::stringify(&result).into())
                        ])]),
                    ),
                    ("structuredContent", result),
                    ("isError", failed.into()),
                ]),
            ))
        }
        _ => Some(error(id, -32601, "Method not found")),
    }
}

fn tool_request(name: &str, arguments: &Value, origin: &str) -> Result<Value, &'static str> {
    let Value::Object(fields) = arguments else {
        return Err("arguments must be an object");
    };
    let (operation, allowed): (&str, &[&str]) = match name {
        "session_create" => (
            "create",
            &["name", "command", "cwd", "cols", "rows", "history"],
        ),
        "session_list" => ("list", &[]),
        "session_read" => ("read", &["session", "cursor", "max_bytes"]),
        "session_write" => ("write", &["session", "data"]),
        "session_resize" => ("resize", &["session", "cols", "rows"]),
        "session_close" => ("close", &["session"]),
        _ => return Err("Unknown tool"),
    };
    if fields
        .iter()
        .any(|(key, _)| !allowed.contains(&key.as_str()))
    {
        return Err("Unknown tool argument");
    }
    let mut request = object([("op", operation.into())]);
    for (key, value) in fields {
        request.set(key, value.clone());
    }
    if operation != "create" && operation != "list" {
        let selector = arguments
            .get("session")
            .and_then(Value::as_str)
            .ok_or("session is required")?;
        if selector.is_empty() || selector.len() > 4096 {
            return Err("invalid session selector");
        }
    }
    if operation == "create" {
        for key in ["name", "cwd"] {
            if let Some(value) = arguments.get(key) {
                let text = value.as_str().ok_or("name and cwd must be strings")?;
                if text.is_empty() || text.contains('\0') {
                    return Err("name and cwd cannot be empty or contain NUL");
                }
            }
        }
        if let Some(command) = arguments.get("command") {
            let values = command.as_array().ok_or("command must be an argv array")?;
            if values.is_empty() || values.len() > 256 {
                return Err("command needs 1..256 arguments");
            }
            if values
                .iter()
                .any(|v| v.as_str().is_none_or(|s| s.contains('\0')))
                || values[0].as_str() == Some("")
            {
                return Err("invalid command argument");
            }
        }
        request.set("origin", origin.into());
        request.set("headless", true.into());
    }
    for (key, minimum, maximum) in [
        ("cols", 2, 512),
        ("rows", 2, 256),
        ("history", 0, 100000),
        ("max_bytes", 1, 65536),
        ("cursor", 0, u64::MAX),
    ] {
        if let Some(value) = arguments.get(key) {
            let number = value
                .as_u64()
                .ok_or("dimensions, history and cursors must be unsigned integers")?;
            if !(minimum..=maximum).contains(&number) {
                return Err("numeric tool argument is outside its range");
            }
        } else if operation == "resize" && (key == "cols" || key == "rows") {
            return Err("cols and rows are required");
        }
    }
    if operation == "write" {
        let data = arguments
            .get("data")
            .and_then(Value::as_str)
            .ok_or("data is required")?;
        if data.len() > 65536 {
            return Err("data exceeds 64 KiB");
        }
    }
    Ok(request)
}

fn stdio() -> Result<i32, String> {
    let mut session = RpcSession::new();
    let mut pending = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut discard = false;
    loop {
        let count = unsafe { read(0, buffer.as_mut_ptr().cast(), buffer.len()) };
        if count == 0 {
            if !pending.is_empty() {
                emit(error(Value::Null, -32700, "unterminated stdio message"));
            }
            return Ok(0);
        }
        if count < 0 {
            if os::errno() == os::EINTR {
                continue;
            }
            return Err("cannot read MCP stdin".into());
        }
        for &byte in &buffer[..count as usize] {
            if byte == b'\n' {
                if discard {
                    discard = false;
                    continue;
                }
                if pending.last() == Some(&b'\r') {
                    pending.pop();
                }
                if !pending.is_empty() {
                    match json::parse(&pending) {
                        Ok(request) => {
                            if let Some(response) = dispatch(&request, &mut session, "mcp/stdio") {
                                emit(response);
                            }
                        }
                        Err(message) => emit(error(Value::Null, -32700, message)),
                    }
                }
                pending.clear();
            } else if !discard {
                if pending.len() == json::MAX_MESSAGE {
                    pending.clear();
                    discard = true;
                    emit(error(Value::Null, -32700, "MCP message exceeds 1 MiB"));
                } else {
                    pending.push(byte);
                }
            }
        }
    }
}

fn emit(response: Value) {
    let mut bytes = json::stringify(&response);
    bytes.push('\n');
    os::print(1, bytes.as_bytes());
}
