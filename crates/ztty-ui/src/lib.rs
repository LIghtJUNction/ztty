#![doc = include_str!("../README.md")]
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::{self, Write};

mod wire;

pub const VERSION: u8 = 1;
pub const PREFIX: &[u8] = b"ZUI;1;";
pub const MAX_FRAME: usize = 65_536;
pub const MAX_NODES: usize = 256;
pub const MAX_DEPTH: usize = 8;
pub const MAX_TEXT: usize = 32_768;
pub const MAX_STRING: usize = 4_096;
pub const MAX_ACTION: usize = 64;
pub const MAX_EXTENT: u16 = 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextStyle {
    #[default]
    Body,
    Heading,
    Muted,
    Code,
}

/// The complete trusted v1 component catalog. No executable expressions or URLs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Column {
        children: Vec<u32>,
    },
    Row {
        children: Vec<u32>,
    },
    Text {
        text: String,
        style: TextStyle,
    },
    /// Integer basis points, 0..=10000. Labels remain ordinary text.
    Progress {
        value: u16,
        label: String,
    },
    Button {
        label: String,
        action: String,
        enabled: bool,
    },
    /// References the owning terminal's KGP image store; never owns pixel bytes.
    /// None follows the current image at this ID; Some pins one content generation.
    Image {
        image_id: u32,
        generation: Option<u64>,
        alt: String,
        cols: u16,
        rows: u16,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub id: u32,
    pub kind: Kind,
}

impl Node {
    pub fn children(&self) -> &[u32] {
        match &self.kind {
            Kind::Column { children } | Kind::Row { children } => children,
            _ => &[],
        }
    }

    fn text_bytes(&self) -> usize {
        match &self.kind {
            Kind::Text { text, .. } => text.len(),
            Kind::Progress { label, .. } => label.len(),
            Kind::Button { label, action, .. } => label.len() + action.len(),
            Kind::Image { alt, .. } => alt.len(),
            _ => 0,
        }
    }
}

/// One bounded, connected tree. Nodes are sorted by ID, not display order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scene {
    pub surface: u64,
    pub revision: u64,
    pub root: u32,
    pub nodes: Vec<Node>,
    /// Explicit negotiation; false scenes never produce action events.
    pub events: bool,
}

impl Scene {
    pub fn node(&self, id: u32) -> Option<&Node> {
        self.nodes
            .binary_search_by_key(&id, |node| node.id)
            .ok()
            .map(|i| &self.nodes[i])
    }
}

/// An intent returned to the host. It never executes a tool, URL, or shell command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub surface: u64,
    pub revision: u64,
    pub node: u32,
    pub action: String,
    pub event_id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    FrameLimit,
    Syntax,
    Utf8,
    UnknownField,
    DuplicateField,
    Version,
    Value,
    Limit,
    Graph,
    Revision,
    Surface,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Updated,
    Capabilities,
}

/// Validates a capability probe without applying a scene on unsupported hosts.
pub fn is_capabilities_request(bytes: &[u8]) -> bool {
    matches!(wire::parse(bytes), Ok(command) if command.op == wire::Op::Capabilities)
}

/// No allocations until the first scene. Revision/event counters never wrap.
#[derive(Default)]
pub struct State {
    scene: Option<Scene>,
    last_revision: u64,
    generation: u64,
    event_id: u64,
}

impl State {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn scene(&self) -> Option<&Scene> {
        self.scene.as_ref()
    }
    /// Host invalidation token, including lifecycle resets that have no wire rev.
    pub fn revision(&self) -> u64 {
        self.generation
    }
    /// Retained after clear/RIS/screen switch to reject replayed scene revisions.
    pub fn last_revision(&self) -> u64 {
        self.last_revision
    }

    /// Drops all presentation and event opt-in without reusing protocol identity.
    pub fn reset(&mut self) {
        if self.scene.take().is_some() {
            self.generation = self.generation.saturating_add(1);
        }
    }

    pub fn apply(&mut self, bytes: &[u8]) -> Result<Change, Error> {
        let command = wire::parse(bytes)?;
        if command.op == wire::Op::Capabilities {
            return Ok(Change::Capabilities);
        }
        let next_generation = self.generation.checked_add(1).ok_or(Error::Revision)?;
        let revision = command.revision.ok_or(Error::Value)?;
        let surface = command.surface.filter(|id| *id != 0).ok_or(Error::Value)?;
        if revision <= self.last_revision {
            return Err(Error::Revision);
        }
        match command.op {
            wire::Op::Replace => {
                let root = command.root.ok_or(Error::Value)?;
                let mut nodes = Vec::with_capacity(command.entries.len());
                for fields in command.entries {
                    nodes.push(fields.into_node(None)?);
                }
                nodes.sort_unstable_by_key(|node| node.id);
                validate(&nodes, &[], root)?;
                self.scene = Some(Scene {
                    surface,
                    revision,
                    root,
                    nodes,
                    events: command.events.unwrap_or(false),
                });
            }
            wire::Op::Update | wire::Op::Clear => {
                let scene = self.scene.as_ref().ok_or(Error::Surface)?;
                if surface != scene.surface {
                    return Err(Error::Surface);
                }
                if command.base_revision != Some(scene.revision) {
                    return Err(Error::Revision);
                }
                if command.op == wire::Op::Clear {
                    self.scene = None;
                } else {
                    // Stage only changed nodes. Unmodified nodes are not cloned; a changed node retains owned properties.
                    let mut updates = Vec::with_capacity(command.entries.len());
                    for fields in command.entries {
                        let id = fields.id.ok_or(Error::Value)?;
                        let index = scene
                            .nodes
                            .binary_search_by_key(&id, |node| node.id)
                            .map_err(|_| Error::Graph)?;
                        updates.push((index, fields.into_node(Some(&scene.nodes[index]))?));
                    }
                    updates.sort_unstable_by_key(|(index, _)| *index);
                    if updates.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                        return Err(Error::Graph);
                    }
                    validate(&scene.nodes, &updates, scene.root)?;
                    let scene = self.scene.as_mut().ok_or(Error::Surface)?;
                    for (index, node) in updates {
                        scene.nodes[index] = node;
                    }
                    scene.revision = revision;
                }
            }
            wire::Op::Capabilities => unreachable!(),
        }
        self.last_revision = revision;
        self.generation = next_generation;
        Ok(Change::Updated)
    }

    /// Host must additionally check live-session ownership and painted hit geometry.
    /// Expected identity is from the displayed hit, never freshly read after click.
    pub fn action(&mut self, node: u32, surface: u64, revision: u64) -> Option<Action> {
        let scene = self.scene.as_ref()?;
        if !scene.events || scene.surface != surface || scene.revision != revision {
            return None;
        }
        let Kind::Button {
            action,
            enabled: true,
            ..
        } = &scene.node(node)?.kind
        else {
            return None;
        };
        let event_id = self.event_id.checked_add(1)?;
        let result = Action {
            surface,
            revision,
            node,
            action: action.clone(),
            event_id,
        };
        self.event_id = event_id;
        Some(result)
    }

    /// A small, deterministic capability response; callers own framing and I/O.
    pub fn write_capabilities(&self, out: &mut impl Write) -> fmt::Result {
        write!(
            out,
            "{{\"v\":1,\"type\":\"capabilities\",\"supported\":true,\"last_rev\":{},\"max_frame\":{},\"max_nodes\":{},\"max_depth\":{},\"max_text\":{},\"max_string\":{},\"max_action\":{},\"surfaces\":1,\"events\":true,\"components\":[\"column\",\"row\",\"text\",\"progress\",\"button\",\"image\"]}}",
            self.last_revision, MAX_FRAME, MAX_NODES, MAX_DEPTH, MAX_TEXT, MAX_STRING, MAX_ACTION
        )
    }
}

fn candidate<'a>(nodes: &'a [Node], updates: &'a [(usize, Node)], index: usize) -> &'a Node {
    match updates.binary_search_by_key(&index, |(index, _)| *index) {
        Ok(i) => &updates[i].1,
        Err(_) => &nodes[index],
    }
}

fn validate(nodes: &[Node], updates: &[(usize, Node)], root: u32) -> Result<(), Error> {
    if nodes.is_empty() || nodes.len() > MAX_NODES {
        return Err(Error::Limit);
    }
    if nodes[0].id == 0 || nodes.windows(2).any(|pair| pair[0].id == pair[1].id) {
        return Err(Error::Graph);
    }
    let mut text = 0;
    for index in 0..nodes.len() {
        text += candidate(nodes, updates, index).text_bytes();
        if text > MAX_TEXT {
            return Err(Error::Limit);
        }
    }
    let mut visited = [false; MAX_NODES];
    visit(nodes, updates, root, 1, &mut visited)?;
    if visited[..nodes.len()].iter().any(|seen| !seen) {
        return Err(Error::Graph);
    }
    Ok(())
}

fn visit(
    nodes: &[Node],
    updates: &[(usize, Node)],
    id: u32,
    depth: usize,
    visited: &mut [bool; MAX_NODES],
) -> Result<(), Error> {
    if depth > MAX_DEPTH {
        return Err(Error::Limit);
    }
    let index = nodes
        .binary_search_by_key(&id, |node| node.id)
        .map_err(|_| Error::Graph)?;
    if core::mem::replace(&mut visited[index], true) {
        return Err(Error::Graph);
    }
    for child in candidate(nodes, updates, index).children() {
        visit(nodes, updates, *child, depth + 1, visited)?;
    }
    Ok(())
}

/// Incremental APC body collector. Invalid/oversized frames remain discarded until
/// the terminal framing layer reaches ST or cancellation; trailing text cannot leak.
#[derive(Default)]
pub struct Frame {
    bytes: Vec<u8>,
    prefix: usize,
    rejected: bool,
}

impl Frame {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn begin(&mut self) {
        self.bytes.clear();
        self.prefix = 0;
        self.rejected = false;
    }
    pub fn abort(&mut self) {
        self.bytes.clear();
        self.rejected = true;
    }
    pub fn push(&mut self, mut bytes: &[u8]) {
        if self.rejected {
            return;
        }
        while self.prefix < PREFIX.len() && !bytes.is_empty() {
            if bytes[0] != PREFIX[self.prefix] {
                self.abort();
                return;
            }
            self.prefix += 1;
            bytes = &bytes[1..];
        }
        if bytes.len() > MAX_FRAME - self.bytes.len() {
            self.abort();
            return;
        }
        let required = self.bytes.len() + bytes.len();
        if required > self.bytes.capacity() {
            let capacity = required.next_power_of_two().min(MAX_FRAME);
            self.bytes.reserve_exact(capacity - self.bytes.len());
        }
        self.bytes.extend_from_slice(bytes);
    }
    pub fn payload(&self) -> Option<&[u8]> {
        (!self.rejected && self.prefix == PREFIX.len()).then_some(self.bytes.as_slice())
    }
}

#[cfg(test)]
extern crate std;
#[cfg(test)]
mod tests;
