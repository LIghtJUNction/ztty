extern crate alloc;
use ztty::actions;

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

#[test]
fn multiword_matching_ignores_case_and_word_order_across_fields() {
    assert_eq!(
        actions::matching(" HISTORY  terminal ")
            .next()
            .unwrap()
            .name,
        "search"
    );
    assert_eq!(
        actions::matching("session SWITCH").next().unwrap().name,
        "sessions"
    );
    assert!(actions::matching("history nonexistent").next().is_none());
    assert!(actions::matching("  \t ").all(|action| action.name != "none"));
    assert!(actions::matches(
        "BUILD stdio 中文",
        &["build", "中文 logs", "ztty://mcp/stdio/run"]
    ));
    assert!(!actions::matches(
        "中文 http",
        &["build", "中文 logs", "ztty://mcp/stdio/run"]
    ));
}

#[test]
fn session_switcher_has_an_overrideable_default_and_truthful_shortcut_hints() {
    let mut bindings = Vec::new();
    assert_eq!(
        actions::key_action(&bindings, b'S' as u64, 5),
        Some("sessions")
    );
    assert_eq!(actions::key_action(&bindings, b's' as u64, 4), None);
    assert_eq!(
        actions::shortcut(&bindings, "sessions").as_deref(),
        Some("Ctrl+Shift+S")
    );
    actions::set(
        &mut bindings,
        actions::parse("ctrl+shift+s = none", false).unwrap(),
    )
    .unwrap();
    assert_eq!(actions::key_action(&bindings, b's' as u64, 5), Some("none"));
    assert_eq!(actions::shortcut(&bindings, "sessions"), None);
    actions::set(
        &mut bindings,
        actions::parse("alt+f12 = sessions", false).unwrap(),
    )
    .unwrap();
    assert_eq!(
        actions::shortcut(&bindings, "sessions").as_deref(),
        Some("Alt+F12")
    );
    assert_eq!(actions::key_action(&bindings, 0xffc9, 8), Some("sessions"));
    actions::set(
        &mut bindings,
        actions::parse("alt+f12 = copy", false).unwrap(),
    )
    .unwrap();
    assert_eq!(actions::shortcut(&bindings, "sessions"), None);
    assert_eq!(
        actions::shortcut(&bindings, "copy").as_deref(),
        Some("Alt+F12")
    );
    assert_eq!(
        actions::shortcut(&bindings, "palette").as_deref(),
        Some("Ctrl+Shift+P")
    );
}
