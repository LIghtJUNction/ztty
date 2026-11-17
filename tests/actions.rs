extern crate alloc;
#[path = "../src/actions.rs"]
mod actions;

#[test]
fn custom_bindings_override_defaults_and_can_disable_them() {
    let mut bindings = Vec::new();
    actions::set(
        &mut bindings,
        actions::parse("ctrl+shift+t = none", false).unwrap(),
    )
    .unwrap();
    assert_eq!(actions::key_action(&bindings, b'T' as u64, 5), Some("none"));
    actions::set(
        &mut bindings,
        actions::parse("ctrl+shift+t = split-right", false).unwrap(),
    )
    .unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(
        actions::key_action(&bindings, b't' as u64, 5 | 2 | 16),
        Some("split-right")
    );
    assert_eq!(actions::key_action(&bindings, b't' as u64, 5 | 8), None);
}

#[test]
fn invalid_keys_modifiers_and_actions_are_rejected() {
    for value in [
        "ctrl+ctrl+t = new-tab",
        "ctrl = new-tab",
        "ctrl+t+a = new-tab",
        "ctrl+t = missing",
        "f36 = copy",
    ] {
        assert!(actions::parse(value, false).is_err(), "{value}");
    }
    assert!(actions::parse("button0 = copy", true).is_err());
    assert!(actions::parse("button12 = copy", true).is_err());
    assert!(actions::parse("button1 = copy", false).is_err());
    assert!(actions::parse("ctrl+x = copy", true).is_err());
}

#[test]
fn mouse_and_function_keys_are_not_ambiguous() {
    for button in 1..=11 {
        let binding = actions::parse(&format!("button{button} = copy"), true).unwrap();
        assert_eq!(actions::parse(&binding.config(), true).unwrap(), binding);
        assert_eq!(
            actions::lookup(&[binding], 0x1000_0000 + button, 0),
            Some("copy")
        );
    }
    let mouse = actions::parse("alt+button2 = paste-primary", true).unwrap();
    assert_eq!(actions::parse(&mouse.config(), true).unwrap(), mouse);
    assert_eq!(
        actions::lookup(&[mouse], 0x1000_0002, 8),
        Some("paste-primary")
    );
    assert_eq!(actions::lookup(&[mouse], 0x1000_0002, 0), None);
    assert_eq!(
        actions::parse("super+f12 = palette", false).unwrap().symbol,
        0xffc9
    );
    assert_eq!(
        actions::key_action(&[], b'+' as u64, 4),
        Some("font-increase")
    );
    assert_eq!(actions::key_action(&[], 0xff55, 1), Some("scroll-up"));
    assert_eq!(actions::matching("history").next().unwrap().name, "search");
}
