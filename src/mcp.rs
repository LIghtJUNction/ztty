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
const HELP: &str = "Usage: ztty mcp [--transport stdio|http] [--host HOST] [--port PORT]\n                [--read-only] [--token TOKEN] [--observer-token TOKEN]\n                [--allow-host AUTHORITY]\n\nDefaults: stdio; HTTP host 127.0.0.1, port 8732, endpoint /mcp.\nUse --port 0 to choose a free port. ZTTY_MCP_TOKEN supplies an operator token;\nZTTY_MCP_OBSERVER_TOKEN supplies an observer token. --read-only limits all\nclients to session and command reads, including operator-token holders.\nNon-loopback HTTP requires a token. --allow-host explicitly permits another\nHTTP Host authority (name:port), for example behind a trusted local proxy.\nHTTP authentication is required whenever either token is configured.\nMCP protocol versions: 2025-11-25, 2025-06-18, 2025-03-26.\nMCP sessions create persistent PTYs; use ztty attach to list and view them.\n";
const TOOLS: &str = r#"[
{"name":"session_create","description":"Create a persistent terminal session in the shared ztty process. Commands are argv; use an explicit shell for shell syntax. Attach the returned URI with ztty attach.","inputSchema":{"type":"object","properties":{"name":{"type":"string","description":"Optional unique name within this MCP transport namespace"},"command":{"type":"array","items":{"type":"string"},"minItems":1,"description":"Executable and arguments; defaults to the configured shell"},"cwd":{"type":"string"},"cols":{"type":"integer","minimum":2,"maximum":512},"rows":{"type":"integer","minimum":2,"maximum":256},"history":{"type":"integer","minimum":0,"maximum":100000}},"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":false,"openWorldHint":true}},
{"name":"session_list","description":"List terminal sessions, canonical ztty:// URIs, process state and attached windows.","inputSchema":{"type":"object","properties":{},"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}},
{"name":"session_commands","description":"List up to 128 retained OSC 133 shell command boundaries, exit codes and availability. Requires shell integration; records refer to the retained terminal history.","inputSchema":{"type":"object","properties":{"session":{"type":"string"}},"required":["session"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}},
{"name":"session_command_read","description":"Read one shell command input or output from retained terminal cells, bounded to 65536 UTF-8 bytes. This is rendered text, not immutable raw stdout. Check complete and truncated.","inputSchema":{"type":"object","properties":{"session":{"type":"string"},"command_id":{"type":"integer","minimum":1},"part":{"type":"string","enum":["input","output"]},"max_bytes":{"type":"integer","minimum":1,"maximum":65536}},"required":["session","command_id"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}},
{"name":"session_read","description":"Read terminal output from a byte cursor and the current screen. max_bytes is at most 65536; a text page may extend by up to 3 bytes to finish a UTF-8 character. Reuse next_cursor; truncated reports expired output.","inputSchema":{"type":"object","properties":{"session":{"type":"string","description":"Canonical ztty:// URI or unambiguous short name"},"cursor":{"type":"integer","minimum":0},"max_bytes":{"type":"integer","minimum":1,"maximum":65536}},"required":["session"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}},
{"name":"session_write","description":"Send up to 65536 UTF-8 bytes or terminal control bytes to a session. Include a newline to submit a shell command; use \\u0003 to interrupt.","inputSchema":{"type":"object","properties":{"session":{"type":"string"},"data":{"type":"string","maxLength":65536}},"required":["session","data"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":true,"openWorldHint":true}},
{"name":"session_resize","description":"Resize a session PTY and screen; attached windows use the new terminal dimensions.","inputSchema":{"type":"object","properties":{"session":{"type":"string"},"cols":{"type":"integer","minimum":2,"maximum":512},"rows":{"type":"integer","minimum":2,"maximum":256}},"required":["session","cols","rows"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":false,"openWorldHint":false}},
{"name":"session_control","description":"Acquire or release exclusive input control. takeover=true explicitly replaces another operator; observers cannot acquire control.","inputSchema":{"type":"object","properties":{"session":{"type":"string"},"action":{"type":"string","enum":["acquire","release"]},"takeover":{"type":"boolean"}},"required":["session","action"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":true,"openWorldHint":false}},
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
    let mut observer_token = os::env(c"ZTTY_MCP_OBSERVER_TOKEN")
        .map(|bytes| core::str::from_utf8(bytes).map(ToString::to_string))
        .transpose()
        .map_err(|_| "ZTTY_MCP_OBSERVER_TOKEN must be UTF-8")?;
    let mut read_only = false;
    let mut allowed_hosts = Vec::new();
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
            "--read-only" => read_only = true,
            "--transport" | "--host" | "--port" | "--token" | "--observer-token"
            | "--allow-host" => {
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
                    "--observer-token" => {
                        observer_token = Some(value.into());
                        http_option = true;
                    }
                    "--allow-host" => {
                        if !http::valid_authority(value) {
                            return Err("--allow-host must be a host:port authority".into());
                        }
                        if allowed_hosts.len() >= 32 {
                            return Err("at most 32 --allow-host entries are supported".into());
                        }
                        allowed_hosts.push(value.to_string());
                        http_option = true;
                    }
                    _ => unreachable!(),
                }
            }
            _ => return Err(format!("unknown MCP argument: {arg}; run ztty mcp --help")),
        }
        index += 1;
    }
    for token in [token.as_deref(), observer_token.as_deref()]
        .into_iter()
        .flatten()
    {
        if token.is_empty() || token.len() > 4096 || token.bytes().any(|b| b <= 0x20 || b >= 0x7f) {
            return Err("MCP bearer token must be 1..4096 visible ASCII bytes".into());
        }
    }
    if token.is_some() && token == observer_token {
        return Err("operator and observer tokens must be different".into());
    }
    match transport {
        "stdio" if http_option => Err(
            "--host, --port, --token, --observer-token and --allow-host require --transport http"
                .into(),
        ),
        "stdio" => stdio(read_only),
        "http" | "streamable-http" => http::run(
            host,
            port,
            token.as_deref(),
            observer_token.as_deref(),
            read_only,
            &allowed_hosts,
        ),
        _ => Err("--transport must be stdio or http".into()),
    }
}

pub(super) struct RpcSession {
    initialized: bool,
    ready: bool,
    pub(super) version: String,
    /// Trusted configuration/authentication result, never a tool argument.
    pub(super) read_only: bool,
    /// Independent of HTTP credentials because owners are public metadata.
    actor: String,
}

impl RpcSession {
    pub(super) fn new(read_only: bool, actor: String) -> Self {
        Self {
            initialized: false,
            ready: false,
            version: VERSIONS[0].into(),
            read_only,
            actor,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MessageKind {
    Request,
    Notification,
    Response,
}

/// Check the envelope before interpreting any method or changing state.
pub(super) fn message_kind(message: &Value) -> Result<MessageKind, ()> {
    if !matches!(message, Value::Object(_))
        || message.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
    {
        return Err(());
    }
    let id = message.get("id");
    if id.is_some_and(|id| match id {
        Value::String(_) => false,
        Value::Number(number) => !integer_number(number),
        _ => true,
    }) {
        return Err(());
    }
    if let Some(method) = message.get("method") {
        if method.as_str().is_none()
            || message.get("result").is_some()
            || message.get("error").is_some()
        {
            return Err(());
        }
        return Ok(if id.is_some() {
            MessageKind::Request
        } else {
            MessageKind::Notification
        });
    }
    if message.get("params").is_some() {
        return Err(());
    }
    match (message.get("result"), message.get("error")) {
        (Some(Value::Object(_)), None) if id.is_some() => Ok(MessageKind::Response),
        (None, Some(Value::Object(fields))) => {
            let code = fields
                .iter()
                .find(|(key, _)| key == "code")
                .map(|(_, value)| value);
            let message = fields
                .iter()
                .find(|(key, _)| key == "message")
                .map(|(_, value)| value);
            if matches!(code, Some(Value::Number(number)) if integer_number(number))
                && message.and_then(Value::as_str).is_some()
            {
                Ok(MessageKind::Response)
            } else {
                Err(())
            }
        }
        _ => Err(()),
    }
}

/// JSON integer values can use decimal/exponent syntax. Check exactly, without
/// float rounding or expanding arbitrarily large exponents/request IDs.
fn integer_number(token: &str) -> bool {
    let token = token.strip_prefix('-').unwrap_or(token);
    let (mantissa, exponent) = token.split_once(['e', 'E']).unwrap_or((token, "0"));
    if mantissa.bytes().all(|byte| matches!(byte, b'0' | b'.')) {
        return true;
    }
    let exponent = match exponent.parse::<i64>() {
        Ok(value) => value,
        Err(_) => return !exponent.starts_with('-'),
    };
    let fraction = mantissa
        .split_once('.')
        .map_or(0, |(_, digits)| digits.len()) as i64;
    let zeroes_needed = fraction.saturating_sub(exponent);
    zeroes_needed <= 0
        || mantissa
            .bytes()
            .rev()
            .filter(|byte| *byte != b'.')
            .take_while(|byte| *byte == b'0')
            .count() as i64
            >= zeroes_needed
}

fn observable_tool(name: &str) -> bool {
    matches!(
        name,
        "session_list" | "session_read" | "session_commands" | "session_command_read"
    )
}

pub(super) fn dispatch(message: &Value, session: &mut RpcSession, origin: &str) -> Option<Value> {
    let kind = match message_kind(message) {
        Ok(kind) => kind,
        Err(()) => return Some(error(Value::Null, -32600, "Invalid Request")),
    };
    if kind == MessageKind::Response {
        // This server issues no requests. Never interpret an unsolicited
        // response as an executable request.
        return None;
    }
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    if kind == MessageKind::Notification {
        if message
            .get("params")
            .is_some_and(|params| !matches!(params, Value::Object(_)))
        {
            return None;
        }
        if method == "notifications/initialized" && session.initialized {
            session.ready = true;
        }
        return None;
    }
    let id = message.get("id").cloned();
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
            ("instructions", if session.read_only {
                "Observer access: list sessions and read output. Input, session creation and control changes are denied."
            } else {
                "Create persistent PTYs, read output incrementally, acquire input control, write input, and attach their ztty:// URIs in a desktop window."
            }.into()),
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
            let mut tools = json::parse(TOOLS.as_bytes()).expect("static tool schema");
            if session.read_only
                && let Value::Array(values) = &mut tools
            {
                values.retain(|tool| {
                    tool.get("name")
                        .and_then(Value::as_str)
                        .is_some_and(observable_tool)
                });
            }
            Some(success(id, object([("tools", tools)])))
        }
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(error(id, -32602, "tool name is required"));
            };
            let arguments = params.get("arguments").unwrap_or(&empty);
            let mut request = match tool_request(name, arguments, origin) {
                Ok(request) => request,
                Err(message) => return Some(error(id, -32602, message)),
            };
            if session.read_only && !observable_tool(name) {
                return Some(error(
                    id,
                    -32001,
                    "Observer access does not permit this tool",
                ));
            }
            request.set("_actor", session.actor.clone().into());
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
        "session_commands" => ("commands", &["session"]),
        "session_command_read" => (
            "command-read",
            &["session", "command_id", "part", "max_bytes"],
        ),
        "session_write" => ("write", &["session", "data"]),
        "session_resize" => ("resize", &["session", "cols", "rows"]),
        "session_close" => ("close", &["session"]),
        "session_control" => ("control", &["session", "action", "takeover"]),
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
        ("command_id", 1, u64::MAX),
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
    if operation == "command-read" {
        if arguments.get("command_id").is_none() {
            return Err("command_id is required");
        }
        if let Some(part) = arguments.get("part")
            && !matches!(part.as_str(), Some("input" | "output"))
        {
            return Err("part must be input or output");
        }
    }
    if operation == "control" {
        let action = arguments
            .get("action")
            .and_then(Value::as_str)
            .ok_or("action is required")?;
        if !matches!(action, "acquire" | "release") {
            return Err("action must be acquire or release");
        }
        if let Some(takeover) = arguments.get("takeover") {
            if takeover.as_bool().is_none() {
                return Err("takeover must be a boolean");
            }
            if action != "acquire" {
                return Err("takeover is only valid when acquiring control");
            }
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

fn stdio(read_only: bool) -> Result<i32, String> {
    let actor = http::random_id().map_err(|()| "cannot create secure MCP actor identity")?;
    let mut session = RpcSession::new(read_only, format!("mcp:{actor}"));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn call(message: &str, session: &mut RpcSession) -> Option<Value> {
        dispatch(
            &json::parse(message.as_bytes()).unwrap(),
            session,
            "mcp/stdio",
        )
    }

    fn ready(read_only: bool) -> RpcSession {
        let mut session = RpcSession::new(read_only, "trusted-test-actor".into());
        session.initialized = true;
        session.ready = true;
        session
    }

    fn code(value: &Value) -> Option<i64> {
        value
            .get("error")
            .and_then(|value| value.get("code"))
            .and_then(Value::as_i64)
    }

    #[test]
    fn request_ids_are_exact_integers_without_float_rounding() {
        for number in [
            "1.0",
            "1e0",
            "1.25e2",
            "100e-2",
            "0e-99999999999999999999999",
            "9007199254740993123456789",
            "1e99999999999999999999999999",
        ] {
            assert!(integer_number(number), "{number}");
        }
        for number in [
            "1.5",
            "1.00000000000000001",
            "1e-1",
            "1e-99999999999999999999999",
        ] {
            assert!(!integer_number(number), "{number}");
        }
    }

    #[test]
    fn malformed_envelopes_never_run_requests() {
        let mut session = ready(false);
        for message in [
            r#"{"jsonrpc":"2.0","id":1,"method":"ping","result":{}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"ping","error":{"code":1,"message":"x"}}"#,
            r#"{"jsonrpc":"2.0","id":1.5,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":true,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":1,"result":{},"error":{"code":1,"message":"x"}}"#,
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":"bad","message":"x"}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":5}"#,
            r#"{"jsonrpc":"2.0","result":{}}"#,
            "[]",
            "null",
        ] {
            assert_eq!(
                code(&call(message, &mut session).unwrap()),
                Some(-32600),
                "{message}"
            );
        }
        assert!(call(r#"{"jsonrpc":"2.0","id":2,"result":{}}"#, &mut session).is_none());
        assert!(
            call(
                r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"unknown"}}"#,
                &mut session
            )
            .is_none()
        );
    }

    #[test]
    fn uncorrelated_error_responses_are_accepted_without_dispatch() {
        let mut session = ready(false);
        for message in [
            r#"{"jsonrpc":"2.0","error":{"code":-32700,"message":"parse error"}}"#,
            r#"{"jsonrpc":"2.0","id":"unused","error":{"code":-32601.0,"message":"unknown"}}"#,
        ] {
            let value = json::parse(message.as_bytes()).unwrap();
            assert_eq!(message_kind(&value), Ok(MessageKind::Response));
            assert!(dispatch(&value, &mut session, "test").is_none());
        }
        let value =
            json::parse(br#"{"jsonrpc":"2.0","error":{"code":0.1,"message":"invalid"}}"#).unwrap();
        assert_eq!(message_kind(&value), Err(()));
    }

    #[test]
    fn malformed_ready_notification_does_not_advance_lifecycle() {
        let mut session = RpcSession::new(false, "actor".into());
        session.initialized = true;
        assert!(
            call(
                r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":null}"#,
                &mut session
            )
            .is_none()
        );
        assert!(!session.ready);
        assert!(
            call(
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                &mut session
            )
            .is_none()
        );
        assert!(session.ready);
    }

    #[test]
    fn observer_catalog_and_dispatch_enforce_same_permissions() {
        let mut session = ready(true);
        let listed = call(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
            &mut session,
        )
        .unwrap();
        let names: Vec<_> = listed
            .get("result")
            .unwrap()
            .get("tools")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.get("name").unwrap().as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "session_list",
                "session_commands",
                "session_command_read",
                "session_read"
            ]
        );
        for (name, arguments) in [
            ("session_create", object([])),
            (
                "session_write",
                object([("session", "test".into()), ("data", "x".into())]),
            ),
            (
                "session_resize",
                object([
                    ("session", "test".into()),
                    ("cols", 80u32.into()),
                    ("rows", 24u32.into()),
                ]),
            ),
            ("session_close", object([("session", "test".into())])),
            (
                "session_control",
                object([
                    ("session", "test".into()),
                    ("action", "acquire".into()),
                    ("takeover", true.into()),
                ]),
            ),
        ] {
            let message = object([
                ("jsonrpc", "2.0".into()),
                ("id", 2i32.into()),
                ("method", "tools/call".into()),
                (
                    "params",
                    object([("name", name.into()), ("arguments", arguments)]),
                ),
            ]);
            assert_eq!(
                code(&dispatch(&message, &mut session, "mcp/stdio").unwrap()),
                Some(-32001),
                "{name}"
            );
        }
        let read = call(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"session_read","arguments":{"session":"test"}}}"#, &mut session).unwrap();
        assert_eq!(
            read.get("result")
                .unwrap()
                .get("structuredContent")
                .unwrap()
                .get("_actor")
                .unwrap()
                .as_str(),
            Some("trusted-test-actor")
        );
    }

    #[test]
    fn actor_cannot_be_forged_by_tool_arguments() {
        let mut session = ready(false);
        let value = call(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"session_write","arguments":{"session":"test","data":"x","_actor":"victim"}}}"#, &mut session).unwrap();
        assert_eq!(code(&value), Some(-32602));
    }

    #[test]
    fn input_control_maps_only_explicit_valid_actions() {
        let mut session = ready(false);
        let result = call(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"session_control","arguments":{"session":"test","action":"acquire","takeover":true}}}"#, &mut session).unwrap();
        let request = result
            .get("result")
            .unwrap()
            .get("structuredContent")
            .unwrap();
        assert_eq!(request.get("op").unwrap().as_str(), Some("control"));
        assert_eq!(
            request.get("_actor").unwrap().as_str(),
            Some("trusted-test-actor")
        );
        for arguments in [
            r#"{"session":"test","action":"steal"}"#,
            r#"{"session":"test","action":"release","takeover":true}"#,
            r#"{"session":"test","action":"acquire","takeover":"true"}"#,
            r#"{"session":"test"}"#,
        ] {
            assert!(
                tool_request(
                    "session_control",
                    &json::parse(arguments.as_bytes()).unwrap(),
                    "mcp/stdio"
                )
                .is_err()
            );
        }
    }
    #[test]
    fn command_reads_validate_ids_parts_and_bounds_for_observers() {
        for arguments in [
            r#"{"session":"test"}"#,
            r#"{"session":"test","command_id":0}"#,
            r#"{"session":"test","command_id":1.5}"#,
            r#"{"session":"test","command_id":1,"part":"raw"}"#,
            r#"{"session":"test","command_id":1,"max_bytes":65537}"#,
            r#"{"session":"test","command_id":1,"_actor":"local"}"#,
        ] {
            assert!(
                tool_request(
                    "session_command_read",
                    &json::parse(arguments.as_bytes()).unwrap(),
                    "mcp/stdio"
                )
                .is_err()
            );
        }
        let mut session = ready(true);
        let result=call(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"session_command_read","arguments":{"session":"test","command_id":1,"part":"input","max_bytes":32}}}"#,&mut session).unwrap();
        let request = result
            .get("result")
            .unwrap()
            .get("structuredContent")
            .unwrap();
        assert_eq!(request.get("op").unwrap().as_str(), Some("command-read"));
        assert_eq!(
            request.get("_actor").unwrap().as_str(),
            Some("trusted-test-actor")
        );
        assert_eq!(request.get("max_bytes").unwrap().as_u64(), Some(32));
    }
}
