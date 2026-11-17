//! Versioned workspace snapshots referring only to existing session instances.
//!
//! Trees use a flat postorder node array so a valid 64-pane tree also fits the
//! JSON parser's nesting limit. Child indices must precede their parent, every
//! node is consumed once, and all nodes must belong to a tab. This rejects
//! cycles, shared subtrees and unreachable data without recursive JSON input.
//! Loading performs no I/O, spawns no sessions and mutates no live workspace.
use alloc::{boxed::Box, string::String, vec::Vec};

use crate::{
    json::{self, Value},
    layout::{Axis, Layout, MAX_PANES, MAX_TABS, Node},
};

// URI-only version 1 cannot distinguish a closed/recreated named session.
const VERSION: u64 = 2;
const MAX_NODES: usize = MAX_PANES * 2 - 1;
const MAX_DEPTH: usize = 64;
const MAX_URI_BYTES: usize = 2048;
const MAX_EXTENT: u64 = 32767;
// Accommodates an 8K desktop while bounding requested backing-store area.
const MAX_PIXELS: u64 = 64 * 1024 * 1024;

/// Identity of one live session at export time. These values prevent restoring
/// a layout into a new PTY that later reused the same name; they are not an
/// authentication credential. The resolver must verify every value on import.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionIdentity {
    pub uri: String,
    pub id: u64,
    pub created_at: u64,
    pub broker_pid: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub layout: Layout,
    pub width: i32,
    pub height: i32,
}

fn dimensions(width: u64, height: u64) -> Result<(i32, i32), &'static str> {
    if width == 0 || height == 0 || width > MAX_EXTENT || height > MAX_EXTENT {
        return Err("workspace dimensions must be 1..32767 pixels");
    }
    if width * height > MAX_PIXELS {
        return Err("workspace dimensions exceed the 64-megapixel area limit");
    }
    Ok((width as i32, height as i32))
}

fn uri(value: &str) -> Result<(), &'static str> {
    if value.len() > MAX_URI_BYTES || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("invalid workspace session URI");
    }
    let (origin, session) = value
        .strip_prefix("ztty://")
        .and_then(|v| v.split_once('/'))
        .ok_or("workspace sessions require complete ztty:// URIs")?;
    if origin.is_empty() || session.is_empty() {
        return Err("workspace sessions require an origin and session name");
    }
    Ok(())
}

fn fields(value: &Value, expected: &[&str]) -> Result<(), &'static str> {
    let Value::Object(entries) = value else {
        return Err("workspace objects require named fields");
    };
    if entries.len() != expected.len() {
        return Err("workspace object contains missing or unexpected fields");
    }
    for (index, (key, _)) in entries.iter().enumerate() {
        if !expected.contains(&key.as_str()) || entries[..index].iter().any(|(old, _)| old == key) {
            return Err("workspace object contains duplicate or unexpected fields");
        }
    }
    Ok(())
}

fn integer(value: &Value, key: &str) -> Result<u64, &'static str> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or("workspace fields require nonnegative integer values")
}

fn ratio(value: u64) -> Result<u16, &'static str> {
    if !(100..=900).contains(&value) {
        return Err("workspace split ratios must be 100..900 parts per thousand");
    }
    Ok(value as u16)
}

struct Exporter<F> {
    resolve: F,
    nodes: Vec<Value>,
    // Node indices avoid retaining a second copy of every URI.
    panes: Vec<(u64, usize)>,
    focused: u64,
}

impl<F: FnMut(u64) -> Option<SessionIdentity>> Exporter<F> {
    fn node(&mut self, node: &Node, depth: usize) -> Result<(usize, bool), &'static str> {
        if depth > MAX_DEPTH {
            return Err("workspace tree depth exceeds 64");
        }
        let (value, focused) = match node {
            Node::Pane(id) => {
                if *id == 0 || self.panes.iter().any(|(previous, _)| previous == id) {
                    return Err("workspace contains an invalid or duplicate session ID");
                }
                if self.panes.len() >= MAX_PANES {
                    return Err("workspace pane limit exceeded (64)");
                }
                let identity =
                    (self.resolve)(*id).ok_or("workspace session is no longer available")?;
                if identity.id != *id || identity.created_at == 0 || identity.broker_pid == 0 {
                    return Err("workspace session identity is invalid or does not match its pane");
                }
                uri(&identity.uri)?;
                if self.panes.iter().any(|(_, index)| {
                    self.nodes[*index].get("pane").and_then(Value::as_str)
                        == Some(identity.uri.as_str())
                }) {
                    return Err("workspace contains duplicate session URIs");
                }
                self.panes.push((*id, self.nodes.len()));
                (
                    json::object([
                        ("pane", Value::from(identity.uri)),
                        ("id", Value::from(identity.id)),
                        ("created_at", Value::from(identity.created_at)),
                        ("broker_pid", Value::from(identity.broker_pid)),
                    ]),
                    *id == self.focused,
                )
            }
            Node::Split {
                axis,
                ratio: split,
                first,
                second,
            } => {
                let split = ratio(u64::from(*split))?;
                let (first, first_focused) = self.node(first, depth + 1)?;
                let (second, second_focused) = self.node(second, depth + 1)?;
                (
                    json::object([
                        (
                            "axis",
                            Value::from(match axis {
                                Axis::Horizontal => "horizontal",
                                Axis::Vertical => "vertical",
                            }),
                        ),
                        ("ratio", Value::from(split)),
                        ("first", Value::from(first)),
                        ("second", Value::from(second)),
                    ]),
                    first_focused || second_focused,
                )
            }
        };
        if self.nodes.len() >= MAX_NODES {
            return Err("workspace node limit exceeded (127)");
        }
        let index = self.nodes.len();
        self.nodes.push(value);
        Ok((index, focused))
    }
}

/// Serialize a validated layout. The resolver supplies each live pane's stable
/// identity once; missing sessions, duplicate IDs/URIs and invalid state fail.
pub fn export(
    layout: &Layout,
    width: i32,
    height: i32,
    identity_for: impl FnMut(u64) -> Option<SessionIdentity>,
) -> Result<Value, &'static str> {
    let (width, height) = dimensions(
        u64::try_from(width).map_err(|_| "workspace width must be positive")?,
        u64::try_from(height).map_err(|_| "workspace height must be positive")?,
    )?;
    if layout.tabs.is_empty() || layout.tabs.len() > MAX_TABS {
        return Err("workspace requires 1..32 tabs");
    }
    if layout.active_tab >= layout.tabs.len() {
        return Err("workspace active tab is outside the tab list");
    }
    let mut writer = Exporter {
        resolve: identity_for,
        nodes: Vec::new(),
        panes: Vec::new(),
        focused: layout.focused,
    };
    let mut tabs = Vec::with_capacity(layout.tabs.len());
    let mut active_has_focus = false;
    for (index, tab) in layout.tabs.iter().enumerate() {
        let (node, focused) = writer.node(tab, 1)?;
        tabs.push(Value::from(node));
        if index == layout.active_tab {
            active_has_focus = focused;
        }
    }
    if !active_has_focus {
        return Err("workspace focused session must belong to the active tab");
    }
    let focused = writer
        .panes
        .iter()
        .find(|(id, _)| *id == layout.focused)
        .and_then(|(_, index)| writer.nodes[*index].get("pane"))
        .cloned()
        .ok_or("workspace focused session is missing")?;
    Ok(json::object([
        ("version", Value::from(VERSION)),
        ("width", Value::from(width)),
        ("height", Value::from(height)),
        ("tabs", Value::Array(tabs)),
        ("active_tab", Value::from(layout.active_tab)),
        ("focused", focused),
        ("maximized", Value::Bool(layout.maximized)),
        ("nodes", Value::Array(writer.nodes)),
    ]))
}

struct Built {
    node: Option<Node>,
    depth: usize,
    focused: bool,
}

/// Validate and rebuild a layout using only sessions known to the resolver.
/// The resolver receives (URI, saved ID, creation time, broker PID) and must
/// return the live ID only when all four still identify the same session.
/// The returned value can be applied after complete validation; this function
/// never starts, attaches, closes or otherwise changes a live session.
pub fn import(
    value: &Value,
    mut id_for: impl FnMut(&str, u64, u64, u32) -> Option<u64>,
) -> Result<Snapshot, &'static str> {
    fields(
        value,
        &[
            "version",
            "width",
            "height",
            "tabs",
            "active_tab",
            "focused",
            "maximized",
            "nodes",
        ],
    )?;
    if integer(value, "version")? != VERSION {
        return Err("unsupported workspace snapshot version");
    }
    let (width, height) = dimensions(integer(value, "width")?, integer(value, "height")?)?;
    let tabs = value
        .get("tabs")
        .and_then(Value::as_array)
        .ok_or("workspace tabs must be an array")?;
    if tabs.is_empty() || tabs.len() > MAX_TABS {
        return Err("workspace requires 1..32 tabs");
    }
    let active_tab = integer(value, "active_tab")?;
    if active_tab >= tabs.len() as u64 {
        return Err("workspace active tab is outside the tab list");
    }
    let active_tab = active_tab as usize;
    let focused = value
        .get("focused")
        .and_then(Value::as_str)
        .ok_or("workspace focused session must be a URI")?;
    uri(focused)?;
    let maximized = value
        .get("maximized")
        .and_then(Value::as_bool)
        .ok_or("workspace maximized must be a boolean")?;
    let nodes = value
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or("workspace nodes must be an array")?;
    if nodes.is_empty() || nodes.len() > MAX_NODES {
        return Err("workspace requires 1..127 nodes");
    }
    let mut built: Vec<Built> = Vec::with_capacity(nodes.len());
    let mut panes: Vec<(&str, u64)> = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        if let Some(address) = node.get("pane") {
            fields(node, &["pane", "id", "created_at", "broker_pid"])?;
            let address = address
                .as_str()
                .ok_or("workspace pane must be a session URI")?;
            uri(address)?;
            if panes.len() >= MAX_PANES {
                return Err("workspace pane limit exceeded (64)");
            }
            if panes.iter().any(|(previous, _)| *previous == address) {
                return Err("workspace contains duplicate session URIs");
            }
            let saved_id = integer(node, "id")?;
            let created_at = integer(node, "created_at")?;
            let broker_pid = u32::try_from(integer(node, "broker_pid")?)
                .map_err(|_| "workspace broker PID exceeds its integer range")?;
            if saved_id == 0 || created_at == 0 || broker_pid == 0 {
                return Err("workspace session identity values must be nonzero");
            }
            if panes.iter().any(|(_, previous)| *previous == saved_id) {
                return Err("workspace contains duplicate saved session IDs");
            }
            let id = id_for(address, saved_id, created_at, broker_pid)
                .filter(|id| *id != 0)
                .ok_or("workspace session identity is no longer available")?;
            if panes.iter().any(|(_, previous)| *previous == id) {
                return Err("workspace URIs resolve to the same session more than once");
            }
            if id != saved_id {
                return Err("workspace session ID changed since the snapshot was saved");
            }
            panes.push((address, id));
            built.push(Built {
                node: Some(Node::Pane(id)),
                depth: 1,
                focused: address == focused,
            });
        } else {
            fields(node, &["axis", "ratio", "first", "second"])?;
            let axis = match node.get("axis").and_then(Value::as_str) {
                Some("horizontal") => Axis::Horizontal,
                Some("vertical") => Axis::Vertical,
                _ => return Err("workspace split axis must be horizontal or vertical"),
            };
            let ratio = ratio(integer(node, "ratio")?)?;
            let first = integer(node, "first")?;
            let second = integer(node, "second")?;
            if first >= index as u64 || second >= index as u64 || first == second {
                return Err("workspace split children must be distinct preceding nodes");
            }
            let (first, second) = (first as usize, second as usize);
            let depth = built[first].depth.max(built[second].depth) + 1;
            if depth > MAX_DEPTH {
                return Err("workspace tree depth exceeds 64");
            }
            let focused = built[first].focused || built[second].focused;
            let first = built[first]
                .node
                .take()
                .ok_or("workspace nodes cannot have multiple parents")?;
            let second = built[second]
                .node
                .take()
                .ok_or("workspace nodes cannot have multiple parents")?;
            built.push(Built {
                node: Some(Node::Split {
                    axis,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                depth,
                focused,
            });
        }
    }
    let mut roots = Vec::with_capacity(tabs.len());
    for (tab_index, root) in tabs.iter().enumerate() {
        let root = root
            .as_u64()
            .filter(|root| *root < built.len() as u64)
            .ok_or("workspace tab refers to an invalid node")? as usize;
        if tab_index == active_tab && !built[root].focused {
            return Err("workspace focused session must belong to the active tab");
        }
        roots.push(
            built[root]
                .node
                .take()
                .ok_or("workspace tabs must refer to distinct tree roots")?,
        );
    }
    if built.iter().any(|node| node.node.is_some()) {
        return Err("workspace contains nodes outside its tabs");
    }
    let focused = panes
        .iter()
        .find(|(address, _)| *address == focused)
        .map(|(_, id)| *id)
        .ok_or("workspace focused session is missing")?;
    Ok(Snapshot {
        layout: Layout {
            tabs: roots,
            active_tab,
            focused,
            maximized,
        },
        width,
        height,
    })
}
