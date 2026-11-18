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
const HELP: &str = "Usage: ztty mcp [--transport stdio|http] [--host HOST] [--port PORT]\n                [--read-only] [--token TOKEN] [--observer-token TOKEN]\n                [--allow-host AUTHORITY] [--http-response json|sse]\n\nDefaults: stdio; HTTP host 127.0.0.1, port 8732, endpoint /mcp.\nHTTP replies default to JSON. --http-response sse streams each POST request\nwith keepalive comments and one final response; GET subscriptions are not provided.\nUse --port 0 to choose a free port. ZTTY_MCP_TOKEN supplies an operator token;\nZTTY_MCP_OBSERVER_TOKEN supplies an observer token. --read-only limits all\nclients to session and command reads, including operator-token holders.\nNon-loopback HTTP requires a token. --allow-host explicitly permits another\nHTTP Host authority (name:port), for example behind a trusted local proxy.\nHTTP authentication is required whenever either token is configured.\nMCP protocol versions: 2025-11-25, 2025-06-18, 2025-03-26.\nMCP sessions create persistent PTYs; use ztty attach to list and view them.\n";
const MAX_PENDING: usize = 32;
const MAX_WAITING: usize = 24;
const MAX_BUFFERED: usize = 4 * 1024 * 1024;
const OUTPUT_TIMEOUT: u64 = 5000;

const TOOLS: &str = r#"[
{"name":"session_create","description":"Create a persistent terminal session in the shared ztty process. Commands are argv; use an explicit shell for shell syntax. Attach the returned URI with ztty attach.","inputSchema":{"type":"object","properties":{"name":{"type":"string","description":"Optional unique name within this MCP transport namespace"},"command":{"type":"array","items":{"type":"string"},"minItems":1,"description":"Executable and arguments; defaults to the configured shell"},"cwd":{"type":"string"},"cols":{"type":"integer","minimum":2,"maximum":512},"rows":{"type":"integer","minimum":2,"maximum":256},"history":{"type":"integer","minimum":0,"maximum":100000}},"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":false,"openWorldHint":true}},
{"name":"session_list","description":"List terminal sessions, canonical ztty:// URIs, process state and attached windows.","inputSchema":{"type":"object","properties":{},"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}},
{"name":"session_commands","description":"List up to 128 retained OSC 133 shell command boundaries, exit codes and availability. Requires shell integration; records refer to the retained terminal history.","inputSchema":{"type":"object","properties":{"session":{"type":"string"}},"required":["session"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}},
{"name":"session_command_read","description":"Read one shell command input or output from retained terminal cells, bounded to 65536 UTF-8 bytes. This is rendered text, not immutable raw stdout. Check complete and truncated.","inputSchema":{"type":"object","properties":{"session":{"type":"string"},"command_id":{"type":"integer","minimum":1},"part":{"type":"string","enum":["input","output"]},"max_bytes":{"type":"integer","minimum":1,"maximum":65536}},"required":["session","command_id"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}},
{"name":"session_read","description":"Read terminal output from a byte cursor and the current screen. Reuse next_cursor and send expected_session from prior uri/id/created_at/broker_pid fields to reject a replaced session. max_bytes is at most 65536; a text page may extend by up to 3 bytes to finish UTF-8. truncated reports expired output. wait_ms (0..30000, default 0) waits without blocking other requests until output, truncation or EOF; timed_out marks expiry. Only output_eof together with has_more=false means the output stream is drained; running=false alone does not.","inputSchema":{"type":"object","properties":{"session":{"type":"string","description":"Canonical ztty:// URI or unambiguous short name"},"cursor":{"type":"integer","minimum":0},"max_bytes":{"type":"integer","minimum":1,"maximum":65536},"wait_ms":{"type":"integer","minimum":0,"maximum":30000},"expected_session":{"type":"object","description":"Optional identity guard copied from a previous create/list/read response; rejects a new PTY that reused the URI. Public identifiers, not credentials.","properties":{"uri":{"type":"string","minLength":1,"maxLength":2048},"id":{"type":"integer","minimum":1},"created_at":{"type":"integer","minimum":1},"broker_pid":{"type":"integer","minimum":1,"maximum":4294967295}},"required":["uri","id","created_at","broker_pid"],"additionalProperties":false}},"required":["session"],"additionalProperties":false},"annotations":{"readOnlyHint":true,"openWorldHint":false}},
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
    let mut sse = false;
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
            | "--allow-host" | "--http-response" => {
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
                    "--http-response" => {
                        sse = match value {
                            "json" => false,
                            "sse" => true,
                            _ => return Err("--http-response must be json or sse".into()),
                        };
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
            "--host, --port, --token, --observer-token, --allow-host and --http-response require --transport http"
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
            sse,
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

/// Compare validated MCP IDs without changing their original response tokens.
/// Numeric spelling is not identity: 7, 7.0 and 7e0 name the same request.
pub(super) fn request_id_eq(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::String(left), Value::String(right)) => left == right,
        (Value::Number(left), Value::Number(right)) => {
            if left == right {
                return true;
            }
            let left = NumericId::new(left);
            let right = NumericId::new(right);
            left.negative == right.negative
                && left
                    .digits
                    .iter()
                    .filter(|byte| **byte != b'.')
                    .eq(right.digits.iter().filter(|byte| **byte != b'.'))
                && equal_id_exponents(left.exponent, left.shift, right.exponent, right.shift)
        }
        _ => false,
    }
}

/// A nonzero ID is its significant digits times 10^(exponent + shift).
/// Borrow the digits and exponent; even enormous exponents are never expanded.
struct NumericId<'a> {
    negative: bool,
    digits: &'a [u8],
    exponent: &'a str,
    shift: i64,
}

impl<'a> NumericId<'a> {
    fn new(token: &'a str) -> Self {
        let negative = token.starts_with('-');
        let token = token.strip_prefix('-').unwrap_or(token);
        let (mantissa, exponent) = token.split_once(['e', 'E']).unwrap_or((token, "0"));
        let bytes = mantissa.as_bytes();
        let Some(first) = bytes.iter().position(|byte| matches!(byte, b'1'..=b'9')) else {
            return Self {
                negative: false,
                digits: &[],
                exponent: "0",
                shift: 0,
            };
        };
        let last = bytes
            .iter()
            .rposition(|byte| matches!(byte, b'1'..=b'9'))
            .unwrap();
        let fraction = mantissa
            .split_once('.')
            .map_or(0, |(_, digits)| digits.len());
        let trailing = bytes[last + 1..]
            .iter()
            .filter(|byte| **byte != b'.')
            .count();
        Self {
            negative,
            digits: &bytes[first..=last],
            exponent,
            shift: trailing as i64 - fraction as i64,
        }
    }
}

fn equal_id_exponents(left: &str, left_shift: i64, right: &str, right_shift: i64) -> bool {
    if let (Ok(left), Ok(right)) = (left.parse::<i64>(), right.parse::<i64>()) {
        return left as i128 + left_shift as i128 == right as i128 + right_shift as i128;
    }
    // A validated nonzero integer cannot have an exponent below -MAX_MESSAGE.
    // An overflowing exponent is therefore positive; a negative peer cannot
    // bridge that gap with a shift bounded by the original message length.
    if left.starts_with('-') || right.starts_with('-') {
        return false;
    }
    let shift = left_shift - right_shift;
    let (base, expected, mut carry) = if shift >= 0 {
        (left, right, shift as u64)
    } else {
        (right, left, (-shift) as u64)
    };
    let mut expected = expected.trim_start_matches('+').bytes().rev();
    // Add only the small decimal-point adjustment, one exponent digit at a
    // time. This also handles a carry through arbitrarily many nines, in O(n)
    // time and constant extra space, without floating-point rounding.
    for digit in base.trim_start_matches('+').bytes().rev() {
        carry += (digit - b'0') as u64;
        if expected.next().unwrap_or(b'0') - b'0' != (carry % 10) as u8 {
            return false;
        }
        carry /= 10;
    }
    while carry != 0 {
        if expected.next().unwrap_or(b'0') - b'0' != (carry % 10) as u8 {
            return false;
        }
        carry /= 10;
    }
    expected.all(|digit| digit == b'0')
}

fn observable_tool(name: &str) -> bool {
    matches!(
        name,
        "session_list" | "session_read" | "session_commands" | "session_command_read"
    )
}

pub(super) enum Action {
    Tool { id: Value, request: Value },
    Cancel(Value),
}

pub(super) struct Dispatch {
    pub(super) response: Option<Value>,
    pub(super) action: Option<Action>,
}

/// Validation and lifecycle transitions do no broker I/O. Transports own the
/// bounded in-flight requests, so waiting for one PTY never blocks another.
pub(super) fn dispatch(message: &Value, session: &mut RpcSession, origin: &str) -> Dispatch {
    let mut action = None;
    let response = dispatch_message(message, session, origin, &mut action);
    Dispatch { response, action }
}

fn dispatch_message(
    message: &Value,
    session: &mut RpcSession,
    origin: &str,
    action: &mut Option<Action>,
) -> Option<Value> {
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
        } else if method == "notifications/cancelled" && session.ready {
            let params = message.get("params");
            if params
                .and_then(|params| params.get("reason"))
                .is_some_and(|reason| reason.as_str().is_none())
            {
                return None;
            }
            if let Some(id) = params.and_then(|params| params.get("requestId"))
                && valid_id(id)
            {
                *action = Some(Action::Cancel(id.clone()));
            }
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
            *action = Some(Action::Tool { id, request });
            None
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
        "session_read" => (
            "read",
            &[
                "session",
                "cursor",
                "max_bytes",
                "expected_session",
                "wait_ms",
            ],
        ),
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
    if let Some(expected) = arguments.get("expected_session") {
        crate::session_identity::ExpectedSession::parse(expected)?;
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
        ("wait_ms", 0, 30000),
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

fn valid_id(id: &Value) -> bool {
    matches!(id, Value::String(_)) || matches!(id, Value::Number(number) if integer_number(number))
}

pub(super) fn tool_response(id: Value, result: Result<Value, String>) -> Value {
    let (result, failed) = match result {
        Ok(value) => (value, false),
        Err(message) => (object([("error", message.into())]), true),
    };
    success(
        id,
        object([
            (
                "content",
                Value::Array(alloc::vec![object([
                    ("type", "text".into()),
                    ("text", json::stringify(&result).into()),
                ])]),
            ),
            ("structuredContent", result),
            ("isError", failed.into()),
        ]),
    )
}

pub(super) struct PendingCall {
    pub(super) id: Value,
    pub(super) broker: crate::broker::PendingRequest,
    pub(super) waiting: bool,
}

impl PendingCall {
    pub(super) fn new(id: Value, request: &Value) -> Result<Self, Value> {
        let waiting = request.get("op").and_then(Value::as_str) == Some("read")
            && request.get("wait_ms").and_then(Value::as_u64).unwrap_or(0) != 0;
        match crate::broker::PendingRequest::new(request) {
            Ok(broker) => Ok(Self {
                id,
                broker,
                waiting,
            }),
            Err(message) => Err(tool_response(id, Err(message))),
        }
    }

    pub(super) fn advance(&mut self) -> Option<Value> {
        self.broker
            .advance()
            .map(|value| tool_response(self.id.clone(), value))
    }

    pub(super) fn buffered_bytes(&self) -> usize {
        let id_bytes = match &self.id {
            Value::String(id) | Value::Number(id) => id.len(),
            _ => 0,
        };
        self.broker.buffered_bytes() + id_bytes
    }
}

struct NonblockingStdio {
    flags: [i32; 2],
}

impl NonblockingStdio {
    fn new() -> Result<Self, String> {
        let flags = [unsafe { os::fcntl(0, 3) }, unsafe { os::fcntl(1, 3) }];
        if flags.iter().any(|flags| *flags < 0) {
            return Err("cannot inspect MCP stdio flags".into());
        }
        let guard = Self { flags };
        for fd in 0..2 {
            if unsafe { os::fcntl(fd, 4, flags[fd as usize] | 0o4000) } < 0 {
                return Err("cannot configure nonblocking MCP stdio".into());
            }
        }
        Ok(guard)
    }
}

impl Drop for NonblockingStdio {
    fn drop(&mut self) {
        for fd in 0..2 {
            unsafe {
                os::fcntl(fd, 4, self.flags[fd as usize]);
            }
        }
    }
}

#[derive(Default)]
struct Output {
    bytes: Vec<u8>,
    sent: usize,
    deadline: Option<u64>,
}

impl Output {
    fn len(&self) -> usize {
        self.bytes.len() - self.sent
    }

    fn push(&mut self, response: Value) -> Result<(), String> {
        let text = json::stringify(&response);
        if self.len().saturating_add(text.len() + 1) > MAX_BUFFERED {
            return Err("MCP stdout backpressure limit exceeded".into());
        }
        if self.sent != 0 {
            self.bytes.drain(..self.sent);
            self.sent = 0;
        }
        self.bytes.reserve_exact(text.len() + 1);
        self.bytes.extend_from_slice(text.as_bytes());
        self.bytes.push(b'\n');
        self.deadline
            .get_or_insert(os::milliseconds() + OUTPUT_TIMEOUT);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        if self.len() == 0 {
            return Ok(());
        }
        let pending = &self.bytes[self.sent..];
        let n = unsafe { os::write(1, pending.as_ptr().cast(), pending.len().min(65536)) };
        if n > 0 {
            self.sent += n as usize;
            self.deadline = Some(os::milliseconds() + OUTPUT_TIMEOUT);
            if self.sent == self.bytes.len() {
                if self.bytes.capacity() > 65536 {
                    self.bytes = Vec::new();
                } else {
                    self.bytes.clear();
                }
                self.sent = 0;
                self.deadline = None;
            }
        } else if n == 0 || !matches!(os::errno(), 4 | 11) {
            return Err("cannot write MCP stdout".into());
        }
        Ok(())
    }
}

fn stdio(read_only: bool) -> Result<i32, String> {
    let actor = http::random_id().map_err(|()| "cannot create secure MCP actor identity")?;
    let mut session = RpcSession::new(read_only, format!("mcp:{actor}"));
    let _nonblocking = NonblockingStdio::new()?;
    let mut input = Vec::new();
    let mut output = Output::default();
    let mut calls: Vec<PendingCall> = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut discard = false;
    let mut eof = false;
    let mut fds = Vec::new();
    loop {
        let mut index = 0;
        while index < calls.len() {
            if let Some(response) = calls[index].advance() {
                calls.swap_remove(index);
                output.push(response)?;
            } else {
                index += 1;
            }
        }
        let now = os::milliseconds();
        if output.deadline.is_some_and(|deadline| now >= deadline) {
            return Err("MCP stdout reader stopped making progress".into());
        }
        if eof && calls.is_empty() && output.len() == 0 {
            return Ok(0);
        }
        if input.len() + output.len() + calls.iter().map(PendingCall::buffered_bytes).sum::<usize>()
            > MAX_BUFFERED
        {
            return Err("MCP stdio buffer limit exceeded".into());
        }
        fds.clear();
        fds.push(os::PollFd {
            fd: if eof { -1 } else { 0 },
            events: os::POLLIN,
            revents: 0,
        });
        fds.push(os::PollFd {
            fd: 1,
            events: if output.len() == 0 { 0 } else { os::POLLOUT },
            revents: 0,
        });
        for call in &calls {
            if let Some(fd) = call.broker.poll_fd() {
                fds.push(fd);
            }
        }
        let timeout = calls
            .iter()
            .filter_map(|call| call.broker.next_deadline())
            .chain(output.deadline)
            .min()
            .map_or(-1, |deadline| {
                deadline.saturating_sub(now).min(i32::MAX as u64) as i32
            });
        let count = unsafe { os::poll(fds.as_mut_ptr(), fds.len(), timeout) };
        if count < 0 {
            if os::errno() == os::EINTR {
                continue;
            }
            return Err("MCP stdio poll failed".into());
        }
        if fds[1].revents & (os::POLLERR | os::POLLHUP | 32) != 0 {
            return Err("MCP stdout disconnected".into());
        }
        if fds[1].revents & os::POLLOUT != 0 {
            output.flush()?;
        }
        if fds[0].revents & (os::POLLIN | os::POLLHUP | os::POLLERR) == 0 {
            continue;
        }
        for _ in 0..8 {
            let count = unsafe { read(0, buffer.as_mut_ptr().cast(), buffer.len()) };
            if count == 0 {
                eof = true;
                // EOF cancels open-ended observation, while accepted ordinary
                // requests still run and deliver their responses to stdout.
                calls.retain(|call| !call.waiting);
                if !input.is_empty() {
                    output.push(error(Value::Null, -32700, "unterminated stdio message"))?;
                    input.clear();
                }
                break;
            }
            if count < 0 {
                if os::errno() == os::EINTR {
                    continue;
                }
                if os::errno() == 11 {
                    break;
                }
                return Err("cannot read MCP stdin".into());
            }
            for &byte in &buffer[..count as usize] {
                if byte == b'\n' {
                    if discard {
                        discard = false;
                        continue;
                    }
                    if input.last() == Some(&b'\r') {
                        input.pop();
                    }
                    if !input.is_empty() {
                        match json::parse(&input) {
                            Ok(message) => {
                                let duplicate = message_kind(&message) == Ok(MessageKind::Request)
                                    && message.get("id").is_some_and(|id| {
                                        calls.iter().any(|call| request_id_eq(&call.id, id))
                                    });
                                if duplicate {
                                    output.push(error(
                                        message.get("id").unwrap().clone(),
                                        -32600,
                                        "Request ID is already pending",
                                    ))?;
                                } else {
                                    let next = dispatch(&message, &mut session, "mcp/stdio");
                                    if let Some(response) = next.response {
                                        output.push(response)?;
                                    }
                                    match next.action {
                                        Some(Action::Cancel(id)) => {
                                            calls.retain(|call| !request_id_eq(&call.id, &id))
                                        }
                                        Some(Action::Tool { id, request }) => {
                                            let waiting = request
                                                .get("wait_ms")
                                                .and_then(Value::as_u64)
                                                .unwrap_or(0)
                                                != 0;
                                            if calls.len() >= MAX_PENDING
                                                || (waiting
                                                    && calls
                                                        .iter()
                                                        .filter(|call| call.waiting)
                                                        .count()
                                                        >= MAX_WAITING)
                                            {
                                                output.push(error(
                                                    id,
                                                    -32000,
                                                    "Concurrent tool request limit reached",
                                                ))?;
                                            } else {
                                                match PendingCall::new(id, &request) {
                                                    Ok(call) => {
                                                        if input.len()
                                                            + output.len()
                                                            + call.buffered_bytes()
                                                            + calls
                                                                .iter()
                                                                .map(PendingCall::buffered_bytes)
                                                                .sum::<usize>()
                                                            > MAX_BUFFERED
                                                        {
                                                            output.push(error(call.id.clone(), -32000, "Concurrent tool buffer limit reached"))?;
                                                        } else {
                                                            calls.push(call);
                                                        }
                                                    }
                                                    Err(response) => output.push(response)?,
                                                }
                                            }
                                        }
                                        None => {}
                                    }
                                }
                            }
                            Err(message) => output.push(error(Value::Null, -32700, message))?,
                        }
                    }
                    input.clear();
                } else if !discard {
                    if input.len() == json::MAX_MESSAGE {
                        input.clear();
                        discard = true;
                        output.push(error(Value::Null, -32700, "MCP message exceeds 1 MiB"))?;
                    } else {
                        input.push(byte);
                    }
                }
            }
        }
    }
}

// These unit tests inspect validation and the exact broker request. Real
// transport scheduling, cancellation and PTY output are tested end to end.
#[cfg(test)]
fn test_dispatch(message: &Value, session: &mut RpcSession, origin: &str) -> Option<Value> {
    let result = dispatch(message, session, origin);
    match result.action {
        Some(Action::Tool { id, request }) => {
            Some(tool_response(id, crate::broker::request(&request)))
        }
        _ => result.response,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(message: &str, session: &mut RpcSession) -> Option<Value> {
        test_dispatch(
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
    fn numeric_request_ids_match_by_exact_value() {
        for aliases in [
            &["7", "7.0", "7e0", "70e-1", "0.7e1", "0.0700E+0002"][..],
            &["-7", "-7.00", "-70e-1"],
            &["0", "-0", "0.00", "-0e-999999999999999999999"],
            &["123", "1.2300e2", "12300e-2"],
            &["9007199254740993", "90071992547409930e-1"],
            &["1e9223372036854775808", "10e9223372036854775807"],
            &[
                "1e1000000000000000000000000",
                "10e999999999999999999999999",
                "100.00e+0999999999999999999999998",
                "0.1e1000000000000000000000001",
            ],
        ] {
            for left in aliases {
                assert!(integer_number(left), "{left}");
                for right in aliases {
                    assert!(
                        request_id_eq(
                            &json::parse(left.as_bytes()).unwrap(),
                            &json::parse(right.as_bytes()).unwrap(),
                        ),
                        "{left} != {right}",
                    );
                }
            }
        }
    }

    #[test]
    fn distinct_request_ids_do_not_collide() {
        for (left, right) in [
            ("7", "-7"),
            ("0", "1"),
            ("0", "-1"),
            ("7", r#""7""#),
            (r#""7e0""#, r#""7""#),
            ("9007199254740993", "9007199254740992"),
            ("9007199254740993123456789", "9007199254740993123456788"),
            ("1e9223372036854775808", "1e9223372036854775807"),
            ("1e9223372036854775808", "1e-0"),
            ("1e1000000000000000000000000", "1e999999999999999999999999"),
            (
                "10e1000000000000000000000000",
                "1e1000000000000000000000000",
            ),
            ("null", "null"),
            ("true", "true"),
        ] {
            let left_id = json::parse(left.as_bytes()).unwrap();
            let right_id = json::parse(right.as_bytes()).unwrap();
            assert!(!request_id_eq(&left_id, &right_id), "{left} == {right}");
            assert!(!request_id_eq(&right_id, &left_id), "{right} == {left}");
        }
        assert!(request_id_eq(&"".into(), &"".into()));
        assert!(request_id_eq(
            &json::parse(br#""a\u0062""#).unwrap(),
            &"ab".into(),
        ));
    }

    #[test]
    fn request_id_exponent_carries_stay_bounded_by_input_size() {
        let length = json::MAX_MESSAGE / 2;
        let left = Value::Number(format!("1e1{}", "0".repeat(length)));
        let right = Value::Number(format!("10e{}", "9".repeat(length)));
        let different = Value::Number(format!("1e{}", "9".repeat(length)));
        assert!(request_id_eq(&left, &right));
        assert!(request_id_eq(&right, &left));
        assert!(!request_id_eq(&left, &different));
        assert!(!request_id_eq(&different, &left));
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
            assert!(test_dispatch(&value, &mut session, "test").is_none());
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
                code(&test_dispatch(&message, &mut session, "mcp/stdio").unwrap()),
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

    #[test]
    fn guarded_reads_preserve_identity_and_observer_permissions() {
        let expected = object([
            ("uri", "ztty://mcp/stdio/work".into()),
            ("id", 7u64.into()),
            ("created_at", 123456u64.into()),
            ("broker_pid", 42u32.into()),
        ]);
        let arguments = object([
            ("session", "work".into()),
            ("cursor", 13u64.into()),
            ("expected_session", expected.clone()),
        ]);
        let message = object([
            ("jsonrpc", "2.0".into()),
            ("id", 1i32.into()),
            ("method", "tools/call".into()),
            (
                "params",
                object([("name", "session_read".into()), ("arguments", arguments)]),
            ),
        ]);
        let mut observer = ready(true);
        let response = test_dispatch(&message, &mut observer, "mcp/stdio").unwrap();
        let request = response
            .get("result")
            .unwrap()
            .get("structuredContent")
            .unwrap();
        assert_eq!(request.get("expected_session"), Some(&expected));
        assert_eq!(request.get("cursor").and_then(Value::as_u64), Some(13));
        assert_eq!(
            request.get("_actor").and_then(Value::as_str),
            Some("trusted-test-actor")
        );
        assert!(
            tool_request(
                "session_read",
                &object([("session", "work".into())]),
                "mcp/stdio"
            )
            .is_ok()
        );
        assert!(
            tool_request(
                "session_write",
                &object([
                    ("session", "work".into()),
                    ("data", "x".into()),
                    ("expected_session", expected),
                ]),
                "mcp/stdio"
            )
            .is_err()
        );
    }

    #[test]
    fn waiting_reads_are_deferred_and_validate_bounds() {
        let mut session = ready(true);
        for wait in [0u64, 1, 30000] {
            let args = object([("session", "work".into()), ("wait_ms", wait.into())]);
            let message = object([
                ("jsonrpc", "2.0".into()),
                ("id", 7i32.into()),
                ("method", "tools/call".into()),
                (
                    "params",
                    object([("name", "session_read".into()), ("arguments", args)]),
                ),
            ]);
            let result = dispatch(&message, &mut session, "mcp/stdio");
            assert!(result.response.is_none());
            let Some(Action::Tool { id, request }) = result.action else {
                panic!("expected a deferred read")
            };
            assert_eq!(id, 7i32.into());
            assert_eq!(request.get("wait_ms").and_then(Value::as_u64), Some(wait));
            assert_eq!(
                request.get("_actor").and_then(Value::as_str),
                Some("trusted-test-actor")
            );
        }
        for wait in ["-1", "30001", "1.5", "null", "true", "\"1\""] {
            let args =
                json::parse(format!("{{\"session\":\"work\",\"wait_ms\":{wait}}}").as_bytes())
                    .unwrap();
            assert!(
                tool_request("session_read", &args, "mcp/stdio").is_err(),
                "{wait}"
            );
        }
    }

    #[test]
    fn cancellation_requires_a_valid_notification_and_id() {
        let mut session = ready(false);
        for body in [
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"read","reason":"done"}}"#,
        ] {
            let result = dispatch(&json::parse(body.as_bytes()).unwrap(), &mut session, "test");
            assert!(result.response.is_none());
            assert!(matches!(result.action, Some(Action::Cancel(_))));
        }
        for params in [
            "null",
            "{}",
            r#"{"requestId":null}"#,
            r#"{"requestId":true}"#,
            r#"{"requestId":1.5}"#,
            r#"{"requestId":7,"reason":false}"#,
        ] {
            let message = json::parse(format!("{{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{params}}}").as_bytes()).unwrap();
            let result = dispatch(&message, &mut session, "test");
            assert!(result.response.is_none());
            assert!(result.action.is_none());
        }
    }

    #[test]
    fn malformed_read_guards_fail_before_broker_dispatch() {
        for expected in [
            "null",
            "[]",
            "{}",
            r#"{"uri":"ztty://local/work","id":1,"created_at":1}"#,
            r#"{"uri":"ztty://local/work","id":1,"created_at":1,"broker_pid":"42"}"#,
            r#"{"uri":"ztty://local/work","id":1,"created_at":1,"broker_pid":42,"extra":true}"#,
        ] {
            let mut observer = ready(true);
            let message = object([
                ("jsonrpc", "2.0".into()),
                ("id", 1i32.into()),
                ("method", "tools/call".into()),
                (
                    "params",
                    object([
                        ("name", "session_read".into()),
                        (
                            "arguments",
                            object([
                                ("session", "work".into()),
                                (
                                    "expected_session",
                                    json::parse(expected.as_bytes()).unwrap(),
                                ),
                            ]),
                        ),
                    ]),
                ),
            ]);
            assert_eq!(
                code(&test_dispatch(&message, &mut observer, "mcp/stdio").unwrap()),
                Some(-32602)
            );
        }
    }
}
