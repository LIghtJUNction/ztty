extern crate alloc;

// These production modules expose additional operations exercised by their
// own tests; this target specifically covers the snapshot boundary.
#[allow(dead_code)]
#[path = "../src/json.rs"]
mod json;
#[allow(dead_code)]
#[path = "../src/layout.rs"]
mod layout;
#[path = "../src/workspace_state.rs"]
mod workspace_state;

use json::Value;
use layout::{Axis, Layout, MAX_PANES, MAX_TABS, Node};
use workspace_state::{SessionIdentity, export, import};

const CREATED_AT: u64 = 1_700_000_000_001;
const BROKER_PID: u32 = 4242;

fn address(id: u64) -> Option<String> {
    Some(format!("ztty://local/session-{id}"))
}

fn identity(id: u64) -> Option<SessionIdentity> {
    Some(SessionIdentity {
        uri: address(id)?,
        id,
        created_at: CREATED_AT,
        broker_pid: BROKER_PID,
    })
}

fn identifier(uri: &str, id: u64, created_at: u64, broker_pid: u32) -> Option<u64> {
    let current = uri
        .strip_prefix("ztty://local/session-")?
        .parse::<u64>()
        .ok()?;
    (current == id && created_at == CREATED_AT && broker_pid == BROKER_PID).then_some(current)
}

fn pane(id: u64) -> Value {
    json::object([
        ("pane", Value::from(address(id).unwrap())),
        ("id", Value::from(id)),
        ("created_at", Value::from(CREATED_AT)),
        ("broker_pid", Value::from(BROKER_PID)),
    ])
}

fn saved(layout: &Layout) -> Value {
    export(layout, 1200, 800, identity).unwrap()
}

fn array_mut<'a>(value: &'a mut Value, key: &str) -> &'a mut Vec<Value> {
    let Value::Object(fields) = value else {
        panic!("object required")
    };
    let field = &mut fields.iter_mut().find(|(name, _)| name == key).unwrap().1;
    let Value::Array(array) = field else {
        panic!("array required")
    };
    array
}

fn split(first: usize, second: usize) -> Value {
    json::object([
        ("axis", Value::from("vertical")),
        ("ratio", Value::from(500u16)),
        ("first", Value::from(first)),
        ("second", Value::from(second)),
    ])
}

#[test]
fn roundtrip_preserves_nested_tabs_geometry_focus_and_maximization() {
    let mut layout = Layout::new(1);
    layout.split(2, Axis::Vertical).unwrap();
    layout.focus(1);
    layout.split(3, Axis::Horizontal).unwrap();
    layout.adjust(100);
    layout.add_tab(4).unwrap();
    layout.split(5, Axis::Horizontal).unwrap();
    layout.focus(3);
    layout.maximized = true;
    let mut exported = Vec::new();
    let value = export(&layout, 1600, 900, |id| {
        exported.push(id);
        identity(id)
    })
    .unwrap();
    assert_eq!(exported, layout.panes());
    let encoded = json::stringify(&value);
    let decoded = json::parse(encoded.as_bytes()).unwrap();
    let mut resolved = Vec::new();
    let restored = import(&decoded, |uri, id, created_at, broker_pid| {
        resolved.push((uri.to_owned(), id, created_at, broker_pid));
        identifier(uri, id, created_at, broker_pid)
    })
    .unwrap();
    assert_eq!((restored.width, restored.height), (1600, 900));
    assert_eq!(resolved.len(), layout.panes().len());
    assert_eq!(value.get("version").and_then(Value::as_u64), Some(2));
    assert_eq!(
        resolved,
        layout
            .panes()
            .into_iter()
            .map(|id| (address(id).unwrap(), id, CREATED_AT, BROKER_PID))
            .collect::<Vec<_>>()
    );
    assert_eq!(restored.layout, layout);
    assert_eq!(restored.layout.placements(1600, 900).len(), 1);
}

#[test]
fn deepest_supported_tree_stays_within_json_parser_depth() {
    let mut layout = Layout::new(1);
    for id in 2..=MAX_PANES as u64 {
        layout
            .split(
                id,
                if id % 2 == 0 {
                    Axis::Horizontal
                } else {
                    Axis::Vertical
                },
            )
            .unwrap();
    }
    let value = saved(&layout);
    assert_eq!(
        value.get("nodes").unwrap().as_array().unwrap().len(),
        MAX_PANES * 2 - 1
    );
    let bytes = json::stringify(&value);
    let value = json::parse(bytes.as_bytes()).unwrap();
    assert_eq!(import(&value, identifier).unwrap().layout, layout);
    let mut more = layout.clone();
    more.tabs[0] = Node::Split {
        axis: Axis::Vertical,
        ratio: 500,
        first: Box::new(more.tabs[0].clone()),
        second: Box::new(Node::Pane(65)),
    };
    assert!(export(&more, 1200, 800, identity).is_err());
}

#[test]
fn tab_and_pane_limits_are_enforced_in_both_directions() {
    let mut layout = Layout::new(1);
    for id in 2..=MAX_TABS as u64 {
        layout.add_tab(id).unwrap();
    }
    let value = saved(&layout);
    assert_eq!(import(&value, identifier).unwrap().layout, layout);
    layout.tabs.push(Node::Pane(33));
    assert!(export(&layout, 1200, 800, identity).is_err());
    let mut extra_tab = value.clone();
    array_mut(&mut extra_tab, "tabs").push(Value::from(0usize));
    assert!(import(&extra_tab, identifier).is_err());
    let mut no_tabs = value.clone();
    no_tabs.set("tabs", Value::Array(Vec::new()));
    assert!(import(&no_tabs, identifier).is_err());
    let mut too_many_nodes = value.clone();
    too_many_nodes.set("nodes", Value::Array(vec![Value::Null; 128]));
    assert!(import(&too_many_nodes, identifier).is_err());
    fn balanced(first: u64, count: u64) -> Node {
        if count == 1 {
            return Node::Pane(first);
        }
        let half = count / 2;
        Node::Split {
            axis: Axis::Vertical,
            ratio: 500,
            first: Box::new(balanced(first, half)),
            second: Box::new(balanced(first + half, count - half)),
        }
    }
    let oversized = Layout {
        tabs: vec![balanced(1, 65)],
        active_tab: 0,
        focused: 1,
        maximized: false,
    };
    assert!(export(&oversized, 1200, 800, identity).is_err());
    // 65 independent panes are over the pane bound even though there are
    // fewer than the node bound; tab validation remains independently bounded.
    let mut panes = saved(&Layout::new(1));
    panes.set("nodes", Value::Array((1..=65).map(pane).collect()));
    assert!(import(&panes, identifier).is_err());
}

#[test]
fn invalid_dimensions_versions_and_field_types_are_rejected() {
    let layout = Layout::new(1);
    for (width, height) in [(0, 100), (-1, 100), (100, 0), (32768, 100), (32767, 32767)] {
        assert!(export(&layout, width, height, identity).is_err());
    }
    assert!(export(&layout, 7680, 4320, identity).is_ok());
    assert!(export(&layout, 1, 1, identity).is_ok());
    for (key, field) in [
        ("version", Value::from(1u64)),
        ("version", Value::from(3u64)),
        ("version", Value::Null),
        ("width", Value::from(0u64)),
        ("height", Value::from(-1i32)),
        ("width", Value::from(32768u64)),
        ("height", Value::Number("1.5".into())),
        ("width", Value::Number("1e3".into())),
        ("active_tab", Value::from(1u64)),
        ("active_tab", Value::from(u64::MAX)),
        ("maximized", Value::from("false")),
        ("tabs", Value::Bool(true)),
        ("nodes", Value::Null),
    ] {
        let mut value = saved(&layout);
        value.set(key, field);
        assert!(import(&value, identifier).is_err(), "{key}: {value:?}");
    }
    let mut area = saved(&layout);
    area.set("width", Value::from(32767u64));
    area.set("height", Value::from(32767u64));
    assert!(import(&area, identifier).is_err());
}

#[test]
fn focus_must_be_a_live_leaf_in_the_active_tab() {
    let mut layout = Layout::new(1);
    layout.add_tab(2).unwrap();
    let valid = saved(&layout);
    for focus in ["ztty://local/session-1", "ztty://local/session-9"] {
        let mut value = valid.clone();
        value.set("focused", Value::from(focus));
        assert!(import(&value, identifier).is_err());
    }
    layout.focused = 1;
    assert!(export(&layout, 1200, 800, identity).is_err());
    layout.focused = 99;
    assert!(export(&layout, 1200, 800, identity).is_err());
    layout.focused = 2;
    layout.active_tab = 2;
    assert!(export(&layout, 1200, 800, identity).is_err());
}

#[test]
fn duplicate_ids_uris_and_resolver_aliases_cannot_duplicate_a_session() {
    let mut layout = Layout::new(1);
    layout.split(2, Axis::Vertical).unwrap();
    let value = saved(&layout);
    assert!(
        export(&layout, 1200, 800, |id| {
            let mut record = identity(id)?;
            record.uri = address(1)?;
            Some(record)
        })
        .is_err()
    );
    assert_eq!(
        import(&value, |_, _, _, _| Some(1)).unwrap_err(),
        "workspace URIs resolve to the same session more than once"
    );
    let mut duplicate_uri = value.clone();
    array_mut(&mut duplicate_uri, "nodes")[1].set("pane", Value::from(address(1).unwrap()));
    assert!(import(&duplicate_uri, identifier).is_err());
    let mut duplicate_saved_id = value.clone();
    array_mut(&mut duplicate_saved_id, "nodes")[1].set("id", Value::from(1u64));
    assert_eq!(
        import(&duplicate_saved_id, |_, id, _, _| Some(id)).unwrap_err(),
        "workspace contains duplicate saved session IDs"
    );
    let mut duplicate_id = layout.clone();
    if let Node::Split { second, .. } = &mut duplicate_id.tabs[0] {
        **second = Node::Pane(1);
    }
    assert!(export(&duplicate_id, 1200, 800, identity).is_err());
    assert!(export(&Layout::new(0), 1200, 800, identity).is_err());
    assert!(import(&saved(&Layout::new(1)), |_, _, _, _| Some(0)).is_err());
    assert_eq!(
        import(&saved(&Layout::new(1)), |_, id, _, _| Some(id + 1)).unwrap_err(),
        "workspace session ID changed since the snapshot was saved"
    );
}

#[test]
fn a_recreated_session_with_the_same_uri_cannot_replace_the_saved_pty() {
    let layout = Layout::new(1);
    let snapshot = saved(&layout);
    assert_eq!(import(&snapshot, identifier).unwrap().layout, layout);
    let canonical_uri = address(1).unwrap();
    for (live_id, live_created_at, live_broker_pid) in [
        (2, CREATED_AT, BROKER_PID),
        (1, CREATED_AT + 1, BROKER_PID),
        (1, CREATED_AT, BROKER_PID + 1),
        (2, CREATED_AT + 1, BROKER_PID + 1),
    ] {
        let mut requested = None;
        let restored = import(&snapshot, |uri, id, created_at, broker_pid| {
            requested = Some((uri.to_owned(), id, created_at, broker_pid));
            (uri == canonical_uri
                && id == live_id
                && created_at == live_created_at
                && broker_pid == live_broker_pid)
                .then_some(live_id)
        });
        assert_eq!(
            requested,
            Some((canonical_uri.clone(), 1, CREATED_AT, BROKER_PID))
        );
        assert_eq!(
            restored.unwrap_err(),
            "workspace session identity is no longer available"
        );
    }
}

#[test]
fn identity_fields_are_required_nonzero_and_exact_integers() {
    let layout = Layout::new(1);
    for (key, invalid) in [
        ("id", Value::from(0u64)),
        ("created_at", Value::from(0u64)),
        ("broker_pid", Value::from(0u32)),
        ("id", Value::from(-1i32)),
        ("created_at", Value::Number("1.5".into())),
        ("created_at", Value::Number("1e3".into())),
        ("created_at", Value::Number("18446744073709551616".into())),
        ("id", Value::from("1")),
        ("broker_pid", Value::Bool(true)),
        ("broker_pid", Value::from(u64::from(u32::MAX) + 1)),
    ] {
        let mut snapshot = saved(&layout);
        array_mut(&mut snapshot, "nodes")[0].set(key, invalid);
        let mut consulted = false;
        assert!(
            import(&snapshot, |_, _, _, _| {
                consulted = true;
                Some(1)
            })
            .is_err()
        );
        assert!(
            !consulted,
            "invalid {key} reached the live session resolver"
        );
    }
    for missing in ["id", "created_at", "broker_pid"] {
        let mut snapshot = saved(&layout);
        if let Value::Object(fields) = &mut array_mut(&mut snapshot, "nodes")[0] {
            fields.retain(|(key, _)| key != missing);
        }
        assert!(import(&snapshot, identifier).is_err(), "missing {missing}");
    }
    for (id, created_at, broker_pid) in [
        (0, CREATED_AT, BROKER_PID),
        (2, CREATED_AT, BROKER_PID),
        (1, 0, BROKER_PID),
        (1, CREATED_AT, 0),
    ] {
        assert!(
            export(&layout, 1200, 800, |_| Some(SessionIdentity {
                uri: address(1).unwrap(),
                id,
                created_at,
                broker_pid,
            }))
            .is_err()
        );
    }
}

#[test]
fn legacy_uri_only_snapshots_are_rejected_before_session_lookup() {
    let mut legacy = saved(&Layout::new(1));
    array_mut(&mut legacy, "nodes")[0] = json::object([("pane", Value::from(address(1).unwrap()))]);
    // Merely changing the version must not turn an unbound URI into a valid
    // creation identity, even if its current session name still exists.
    for version in [1u64, 2] {
        legacy.set("version", Value::from(version));
        assert!(
            import(&legacy, |_, _, _, _| panic!(
                "legacy identity reached resolver"
            ))
            .is_err()
        );
    }
}

#[test]
fn identity_integer_boundaries_survive_json_without_precision_loss() {
    let layout = Layout::new(u64::MAX);
    let snapshot = export(&layout, 1, 1, |id| {
        Some(SessionIdentity {
            uri: address(id).unwrap(),
            id,
            created_at: u64::MAX,
            broker_pid: u32::MAX,
        })
    })
    .unwrap();
    let encoded = json::stringify(&snapshot);
    let parsed = json::parse(encoded.as_bytes()).unwrap();
    let restored = import(&parsed, |uri, id, created_at, broker_pid| {
        (uri == address(u64::MAX).unwrap()
            && id == u64::MAX
            && created_at == u64::MAX
            && broker_pid == u32::MAX)
            .then_some(id)
    })
    .unwrap();
    assert_eq!(restored.layout, layout);
}

#[test]
fn missing_sessions_and_unsafe_or_oversized_addresses_are_rejected() {
    let layout = Layout::new(1);
    assert!(export(&layout, 1200, 800, |_| None).is_err());
    assert!(import(&saved(&layout), |_, _, _, _| None).is_err());
    for bad in [
        "",
        "https://local/session-1",
        "ztty:///session",
        "ztty://local/",
        "ztty://local/session\0",
        "ztty://local/session\n",
        "ztty://local/two words",
        "ztty://local/\u{2003}",
    ] {
        assert!(
            export(&layout, 1200, 800, |id| {
                let mut record = identity(id)?;
                record.uri = bad.into();
                Some(record)
            })
            .is_err()
        );
        let mut value = saved(&layout);
        value.set("focused", Value::from(bad));
        array_mut(&mut value, "nodes")[0].set("pane", Value::from(bad));
        assert!(import(&value, |_, _, _, _| Some(1)).is_err());
    }
    let bad = format!("ztty://local/{}", "x".repeat(2048));
    assert!(
        export(&layout, 1200, 800, |id| {
            let mut record = identity(id)?;
            record.uri = bad.clone();
            Some(record)
        })
        .is_err()
    );
    let large_id = Layout::new(u64::MAX);
    assert_eq!(
        import(&saved(&large_id), identifier).unwrap().layout,
        large_id
    );
}

#[test]
fn graph_cycles_forward_references_shared_children_and_orphans_are_rejected() {
    let mut layout = Layout::new(1);
    layout.split(2, Axis::Vertical).unwrap();
    let value = saved(&layout);
    for (first, second) in [(0, 0), (0, 2), (2, 0), (0, usize::MAX)] {
        let mut malformed = value.clone();
        array_mut(&mut malformed, "nodes")[2] = split(first, second);
        assert!(import(&malformed, identifier).is_err());
    }
    let mut shared = value.clone();
    let nodes = array_mut(&mut shared, "nodes");
    nodes.push(pane(3));
    nodes.push(split(0, 3));
    shared.set(
        "tabs",
        Value::Array(vec![Value::from(2usize), Value::from(4usize)]),
    );
    assert!(import(&shared, identifier).is_err());
    let mut orphan = value.clone();
    array_mut(&mut orphan, "nodes").push(pane(3));
    assert!(import(&orphan, identifier).is_err());
    for roots in [vec![2usize, 2], vec![0], vec![3]] {
        let mut bad_roots = value.clone();
        bad_roots.set(
            "tabs",
            Value::Array(roots.into_iter().map(Value::from).collect()),
        );
        assert!(import(&bad_roots, identifier).is_err());
    }
}

#[test]
fn invalid_split_ratios_axes_and_unexpected_fields_are_rejected() {
    let mut layout = Layout::new(1);
    layout.split(2, Axis::Vertical).unwrap();
    let value = saved(&layout);
    for bad in [0u16, 99, 901, 1000, u16::MAX] {
        let mut input = value.clone();
        array_mut(&mut input, "nodes")[2].set("ratio", Value::from(bad));
        assert!(import(&input, identifier).is_err());
        let mut model = layout.clone();
        if let Node::Split { ratio, .. } = &mut model.tabs[0] {
            *ratio = bad;
        }
        assert!(export(&model, 1200, 800, identity).is_err());
    }
    for good in [100u16, 900] {
        let mut model = layout.clone();
        if let Node::Split { ratio, .. } = &mut model.tabs[0] {
            *ratio = good;
        }
        assert_eq!(import(&saved(&model), identifier).unwrap().layout, model);
    }
    let mut axis = value.clone();
    array_mut(&mut axis, "nodes")[2].set("axis", Value::from("diagonal"));
    assert!(import(&axis, identifier).is_err());
    let mut extra = value.clone();
    extra.set("command", Value::from("must never be executed"));
    assert!(import(&extra, identifier).is_err());
    let mut unknown_node = value.clone();
    array_mut(&mut unknown_node, "nodes")[0].set("spawn", Value::Bool(true));
    assert!(import(&unknown_node, identifier).is_err());
    let mut duplicate_fields = value;
    if let Value::Object(fields) = &mut duplicate_fields {
        fields
            .iter_mut()
            .find(|(key, _)| key == "height")
            .unwrap()
            .0 = "width".into();
    }
    assert!(import(&duplicate_fields, identifier).is_err());
}
