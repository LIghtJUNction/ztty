use super::*;
use alloc::format;
use alloc::string::ToString;
use alloc::vec;

fn replace(nodes: &str, revision: u64, events: bool) -> String {
    format!(
        r#"{{"v":1,"op":"replace","surface":7,"rev":{revision},"root":1,"events":{events},"nodes":{nodes}}}"#
    )
}

const TREE: &str = r#"[{"id":1,"type":"column","children":[2,3,4,5]},{"id":2,"type":"text","text":"编译 🦀","style":"heading"},{"id":3,"type":"progress","value":2000,"label":"Building"},{"id":4,"type":"button","label":"Retry","action":"build.retry"},{"id":5,"type":"image","image_id":42,"alt":"Preview","cols":12,"rows":5}]"#;

#[test]
fn complete_loop_and_stale_actions() {
    let mut ui = State::new();
    assert!(ui.scene().is_none());
    assert_eq!(
        ui.apply(replace(TREE, 1, true).as_bytes()),
        Ok(Change::Updated)
    );
    let first = ui.action(4, 7, 1).unwrap();
    assert_eq!((first.event_id, first.action.as_str()), (1, "build.retry"));
    let patch = br#"{"v":1,"op":"update","surface":7,"base_rev":1,"rev":2,"set":[{"id":3,"value":10000,"label":"Done"},{"id":4,"action":"build.open"}]}"#;
    assert_eq!(ui.apply(patch), Ok(Change::Updated));
    assert!(ui.action(4, 7, 1).is_none());
    assert!(ui.action(4, 8, 2).is_none());
    let second = ui.action(4, 7, 2).unwrap();
    assert_eq!((second.event_id, second.action.as_str()), (2, "build.open"));
    assert!(
        matches!(&ui.scene().unwrap().node(3).unwrap().kind, Kind::Progress {value:10000, label} if label=="Done")
    );
    assert_eq!(
        ui.apply(br#"{"v":1,"op":"clear","surface":7,"base_rev":2,"rev":3}"#),
        Ok(Change::Updated)
    );
    assert!(ui.scene().is_none());
    assert!(ui.action(4, 7, 2).is_none());
    assert_eq!(
        ui.apply(replace(TREE, 1, true).as_bytes()),
        Err(Error::Revision)
    );
}

#[test]
fn failed_update_is_atomic_and_does_not_allocate_new_unchanged_text() {
    let mut ui = State::new();
    ui.apply(replace(TREE, 1, true).as_bytes()).unwrap();
    let before = ui.scene().unwrap().clone();
    let revision = ui.revision();
    assert_eq!(ui.apply(br#"{"v":1,"op":"update","surface":7,"base_rev":1,"rev":2,"set":[{"id":3,"value":5000},{"id":4,"action":"rm -rf"}]}"#), Err(Error::Value));
    assert_eq!(ui.scene(), Some(&before));
    assert_eq!(ui.revision(), revision);
    let text_ptr = match &ui.scene().unwrap().node(2).unwrap().kind {
        Kind::Text { text, .. } => text.as_ptr(),
        _ => unreachable!(),
    };
    ui.apply(
        br#"{"v":1,"op":"update","surface":7,"base_rev":1,"rev":2,"set":[{"id":3,"value":5000}]}"#,
    )
    .unwrap();
    assert!(
        matches!(&ui.scene().unwrap().node(2).unwrap().kind, Kind::Text {text,..} if text.as_ptr()==text_ptr)
    );
}

#[test]
fn events_require_opt_in_and_enabled_current_button() {
    let mut ui = State::new();
    ui.apply(replace(TREE, 1, false).as_bytes()).unwrap();
    assert!(ui.action(4, 7, 1).is_none());
    ui.apply(replace(TREE, 2, true).as_bytes()).unwrap();
    assert!(ui.action(2, 7, 2).is_none());
    ui.apply(br#"{"v":1,"op":"update","surface":7,"base_rev":2,"rev":3,"set":[{"id":4,"enabled":false}]}"#).unwrap();
    assert!(ui.action(4, 7, 3).is_none());
}

#[test]
fn reset_preserves_revision_and_event_identity() {
    let mut ui = State::new();
    ui.apply(replace(TREE, 5, true).as_bytes()).unwrap();
    assert_eq!(ui.action(4, 7, 5).unwrap().event_id, 1);
    ui.reset();
    assert!(ui.scene().is_none());
    assert_eq!(ui.last_revision(), 5);
    assert_eq!(ui.revision(), 2);
    assert_eq!(
        ui.apply(replace(TREE, 5, true).as_bytes()),
        Err(Error::Revision)
    );
    ui.apply(replace(TREE, 6, true).as_bytes()).unwrap();
    assert_eq!(ui.action(4, 7, 6).unwrap().event_id, 2);
}

#[test]
fn capabilities_are_read_only_and_report_floor() {
    let mut ui = State::new();
    ui.apply(replace(TREE, 8, true).as_bytes()).unwrap();
    assert_eq!(
        ui.apply(br#"{"v":1,"op":"capabilities"}"#),
        Ok(Change::Capabilities)
    );
    assert_eq!(ui.revision(), 1);
    let mut output = String::new();
    ui.write_capabilities(&mut output).unwrap();
    assert!(output.contains("\"last_rev\":8"));
    assert!(output.contains("\"max_frame\":65536"));
}

#[test]
fn malformed_untrusted_payloads_leave_scene_untouched() {
    let mut ui = State::new();
    ui.apply(replace(TREE, 1, true).as_bytes()).unwrap();
    let before = ui.scene().unwrap().clone();
    for payload in [
        "{",
        "{}",
        "{\"v\":1,\"op\":\"capabilities\",}",
        "{\"v\":1,\"v\":1,\"op\":\"capabilities\"}",
        "{\"v\":1,\"\\u0076\":1,\"op\":\"capabilities\"}",
        "{\"v\":01,\"op\":\"capabilities\"}",
        "{\"v\":1.0,\"op\":\"capabilities\"}",
        "{\"v\":1e0,\"op\":\"capabilities\"}",
        "{\"v\":18446744073709551616,\"op\":\"capabilities\"}",
        "{\"v\":1,\"op\":\"capabilities\",\"script\":\"foo()\"}",
        "{\"v\":2,\"op\":\"capabilities\"}",
        "{\"v\":1,\"op\":\"capabilities\"}[]",
    ] {
        assert!(ui.apply(payload.as_bytes()).is_err(), "accepted {payload}");
        assert_eq!(ui.scene(), Some(&before));
    }
    assert_eq!(ui.apply(b"\xff"), Err(Error::Utf8));
    assert_eq!(ui.apply(&vec![b' '; MAX_FRAME + 1]), Err(Error::FrameLimit));
    assert_eq!(ui.scene(), Some(&before));
}

#[test]
fn unicode_escapes_and_control_rejection() {
    let mut ui = State::new();
    ui.apply(
        replace(
            r#"[{"id":1,"type":"text","text":"A\u4e2d\ud83e\udd80\nB"}]"#,
            1,
            false,
        )
        .as_bytes(),
    )
    .unwrap();
    assert!(
        matches!(&ui.scene().unwrap().node(1).unwrap().kind, Kind::Text {text,..} if text=="A中🦀\nB")
    );
    for text in [
        r"\u001b[31m",
        r"\ud800",
        r"\udc00",
        r"\ud800X",
        r"\ud800\u0041",
        r"\q",
        r"\r",
        r"\u009b",
    ] {
        let nodes = format!(r#"[{{"id":1,"type":"text","text":"{text}"}}]"#);
        assert!(
            ui.apply(replace(&nodes, 2, false).as_bytes()).is_err(),
            "accepted {text}"
        );
        assert_eq!(ui.last_revision(), 1);
    }
}

#[test]
fn invalid_graphs_and_property_kinds_are_atomic() {
    let mut ui = State::new();
    ui.apply(replace(TREE, 1, true).as_bytes()).unwrap();
    for nodes in [
        r#"[{"id":1,"type":"column","children":[1]}]"#,
        r#"[{"id":1,"type":"column","children":[2]}]"#,
        r#"[{"id":1,"type":"column","children":[2,2]},{"id":2,"type":"text","text":"x"}]"#,
        r#"[{"id":1,"type":"text","text":"x"},{"id":2,"type":"text","text":"unreachable"}]"#,
        r#"[{"id":1,"type":"text","text":"x"},{"id":1,"type":"text","text":"duplicate"}]"#,
        r#"[{"id":1,"type":"text","text":"x","action":"run"}]"#,
        r#"[{"id":1,"type":"progress","value":10001}]"#,
        r#"[{"id":1,"type":"image","image_id":0}]"#,
        r#"[{"id":1,"type":"image","image_id":2,"cols":0}]"#,
        r#"[{"id":1,"type":"button","label":"X","action":""}]"#,
    ] {
        assert!(ui.apply(replace(nodes, 2, true).as_bytes()).is_err());
        assert_eq!(ui.last_revision(), 1);
    }
    for set in [
        r#"[{"id":1,"children":[2,3,4,5,1]}]"#,
        r#"[{"id":3,"type":"text","text":"no"}]"#,
        r#"[{"id":3,"value":3},{"id":3,"value":4}]"#,
    ] {
        let payload =
            format!(r#"{{"v":1,"op":"update","surface":7,"base_rev":1,"rev":2,"set":{set}}}"#);
        assert!(ui.apply(payload.as_bytes()).is_err());
        assert_eq!(ui.last_revision(), 1);
    }
}

#[test]
fn depth_node_and_text_bounds() {
    let chain = |count: usize| {
        let mut nodes = String::from("[");
        for id in 1..=count {
            if id > 1 {
                nodes.push(',');
            }
            if id < count {
                nodes.push_str(&format!(
                    r#"{{"id":{id},"type":"column","children":[{}]}}"#,
                    id + 1
                ));
            } else {
                nodes.push_str(&format!(r#"{{"id":{id},"type":"text","text":"end"}}"#));
            }
        }
        nodes.push(']');
        nodes
    };
    let mut ui = State::new();
    ui.apply(replace(&chain(MAX_DEPTH), 1, false).as_bytes())
        .unwrap();
    assert_eq!(
        ui.apply(replace(&chain(MAX_DEPTH + 1), 2, false).as_bytes()),
        Err(Error::Limit)
    );
    let too_long = format!(
        r#"[{{"id":1,"type":"text","text":"{}"}}]"#,
        "x".repeat(MAX_STRING + 1)
    );
    assert_eq!(
        ui.apply(replace(&too_long, 2, false).as_bytes()),
        Err(Error::Limit)
    );
    let mut nodes = format!(
        r#"[{{"id":1,"type":"column","children":[{}]}}"#,
        (2u32..=10)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    for id in 2..=10 {
        nodes.push_str(&format!(
            r#",{{"id":{id},"type":"text","text":"{}"}}"#,
            "x".repeat(MAX_STRING)
        ));
    }
    nodes.push(']');
    assert_eq!(
        ui.apply(replace(&nodes, 2, false).as_bytes()),
        Err(Error::Limit)
    );
    let many = (1..=MAX_NODES + 1)
        .map(|id| format!(r#"{{"id":{id},"type":"text","text":"x"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(
        ui.apply(replace(&format!("[{many}]"), 2, false).as_bytes()),
        Err(Error::Limit)
    );
}

#[test]
fn counters_never_wrap() {
    let mut ui = State::new();
    ui.apply(replace(TREE, u64::MAX, true).as_bytes()).unwrap();
    assert_eq!(
        ui.apply(replace(TREE, 1, true).as_bytes()),
        Err(Error::Revision)
    );
    ui.event_id = u64::MAX;
    assert!(ui.action(4, 7, u64::MAX).is_none());
    ui.reset();
    assert_eq!(ui.last_revision(), u64::MAX);
}

#[test]
fn frame_handles_every_split_and_cancellation() {
    let mut bytes = PREFIX.to_vec();
    bytes.extend_from_slice(replace(TREE, 1, true).as_bytes());
    for split in 0..=bytes.len() {
        let mut frame = Frame::new();
        frame.begin();
        frame.push(&bytes[..split]);
        frame.push(&bytes[split..]);
        let mut ui = State::new();
        ui.apply(frame.payload().unwrap()).unwrap();
        assert_eq!(ui.scene().unwrap().nodes.len(), 5);
    }
    let mut frame = Frame::new();
    frame.begin();
    for byte in &bytes {
        frame.push(core::slice::from_ref(byte));
    }
    assert!(frame.payload().is_some());
    frame.abort();
    assert!(frame.payload().is_none());
    frame.begin();
    frame.push(b"ZUI;2;{}");
    assert!(frame.payload().is_none());
    frame.begin();
    frame.push(PREFIX);
    frame.push(&vec![b' '; MAX_FRAME]);
    assert_eq!(frame.payload().unwrap().len(), MAX_FRAME);
    assert!(frame.bytes.capacity() <= MAX_FRAME);
    frame.push(b"x");
    assert!(frame.payload().is_none());
    frame.push(&bytes);
    assert!(frame.payload().is_none());
    frame.begin();
    frame.push(&bytes);
    assert!(frame.payload().is_some());
}

#[test]
fn image_references_have_no_pixels_and_pin_generation() {
    let mut ui = State::new();
    ui.apply(
        replace(
            r#"[{"id":1,"type":"image","image_id":42,"generation":19,"alt":"图"}]"#,
            1,
            false,
        )
        .as_bytes(),
    )
    .unwrap();
    assert!(matches!(
        &ui.scene().unwrap().node(1).unwrap().kind,
        Kind::Image {
            image_id: 42,
            generation: Some(19),
            cols: 16,
            rows: 8,
            ..
        }
    ));
    ui.apply(br#"{"v":1,"op":"update","surface":7,"base_rev":1,"rev":2,"set":[{"id":1,"generation":20}]}"#).unwrap();
    assert!(matches!(
        &ui.scene().unwrap().node(1).unwrap().kind,
        Kind::Image {
            generation: Some(20),
            ..
        }
    ));
}

#[test]
fn maximum_valid_tree_and_text_budget_are_accepted() {
    let mut nodes = format!(
        r#"[{{"id":1,"type":"column","children":[{}]}}"#,
        (2u32..=256)
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    for id in 2..=256 {
        nodes.push_str(&format!(r#",{{"id":{id},"type":"text","text":"x"}}"#));
    }
    nodes.push(']');
    let mut ui = State::new();
    ui.apply(replace(&nodes, 1, false).as_bytes()).unwrap();
    assert_eq!(ui.scene().unwrap().nodes.len(), MAX_NODES);
    let mut nodes = format!(
        r#"[{{"id":1,"type":"column","children":[{}]}}"#,
        (2u32..=9)
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    for id in 2..=9 {
        nodes.push_str(&format!(
            r#",{{"id":{id},"type":"text","text":"{}"}}"#,
            "x".repeat(MAX_STRING)
        ));
    }
    nodes.push(']');
    ui.apply(replace(&nodes, 2, false).as_bytes()).unwrap();
    assert_eq!(
        ui.scene()
            .unwrap()
            .nodes
            .iter()
            .map(Node::text_bytes)
            .sum::<usize>(),
        MAX_TEXT
    );
}
