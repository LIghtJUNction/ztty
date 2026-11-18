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

#[test]
fn shifted_punctuation_bindings_follow_the_active_layout() {
    let bindings = [
        actions::parse("ctrl+shift+? = search", false).unwrap(),
        actions::parse("ctrl+shift+: = palette", false).unwrap(),
        actions::parse("ctrl+shift+{ = sessions", false).unwrap(),
    ];
    for (base, shifted, expected) in [
        ('/', '?', "search"),
        (';', ':', "palette"),
        ('[', '{', "sessions"),
        // The key that produces '?' need not be '/' on another layout.
        (';', '?', "search"),
    ] {
        assert_eq!(
            actions::key_action_with_shifted(&bindings, base as u64, Some(shifted as u64), 5),
            Some(expected)
        );
        assert_eq!(
            actions::key_action_with_shifted(
                &bindings,
                base as u64,
                Some(shifted as u64),
                5 | 2 | 16
            ),
            Some(expected)
        );
        assert_eq!(
            actions::key_action_with_shifted(&bindings, base as u64, Some(shifted as u64), 4),
            None
        );
        assert_eq!(
            actions::key_action_with_shifted(&bindings, base as u64, Some(shifted as u64), 5 | 8),
            None
        );
    }
}

#[test]
fn shifted_custom_binding_wins_and_unshifted_defaults_remain_available() {
    let mut bindings = vec![
        actions::parse("ctrl+shift+/ = palette", false).unwrap(),
        actions::parse("ctrl+shift+? = none", false).unwrap(),
    ];
    assert_eq!(
        actions::key_action_with_shifted(&bindings, b'/' as u64, Some(b'?' as u64), 5),
        Some("none")
    );
    bindings.pop();
    assert_eq!(
        actions::key_action_with_shifted(&bindings, b'/' as u64, Some(b'?' as u64), 5),
        Some("palette")
    );
    assert_eq!(
        actions::key_action_with_shifted(&[], b'f' as u64, Some(b'F' as u64), 5),
        Some("search")
    );
    assert_eq!(
        actions::key_action_with_shifted(&[], b'=' as u64, None, 4),
        Some("font-increase")
    );
    assert_eq!(
        actions::key_action_with_shifted(&[], b'+' as u64, None, 4),
        Some("font-increase")
    );
}

#[test]
fn common_zoom_aliases_ignore_locks_and_keep_split_resize_distinct() {
    for (symbol, modifiers, expected) in [
        (b'=' as u64, 4, "font-increase"),
        (b'+' as u64, 5, "font-increase"),
        (b'=' as u64, 5, "font-increase"),
        (b'-' as u64, 4, "font-decrease"),
        (b'_' as u64, 5, "font-decrease"),
        (b'-' as u64, 5, "font-decrease"),
        (b'0' as u64, 4, "font-reset"),
        (b'0' as u64, 5, "font-reset"),
        (0xffab, 4, "font-increase"),
        (0xffad, 4, "font-decrease"),
        (0xffb0, 4, "font-reset"),
        (b'+' as u64, 13, "grow"),
        (b'_' as u64, 13, "shrink"),
    ] {
        for locks in [0, 2, 16, 2 | 16] {
            assert_eq!(
                actions::key_action(&[], symbol, modifiers | locks),
                Some(expected)
            );
        }
        assert_eq!(actions::key_action(&[], symbol, modifiers | 64), None);
    }
    assert_eq!(
        actions::key_action(&[], 0xff9e, 4),
        None,
        "Num Lock off keypad Insert remains application input"
    );
    assert_eq!(actions::key_action(&[], b'=' as u64, 4 | 8), None);
    assert_eq!(
        actions::shortcut(&[], "grow").as_deref(),
        Some("Ctrl+Shift+Alt+Equal")
    );
    assert_eq!(
        actions::shortcut(&[], "shrink").as_deref(),
        Some("Ctrl+Shift+Alt+Minus")
    );
}

#[test]
fn default_zoom_uses_shifted_layout_with_no_custom_bindings() {
    assert_eq!(
        actions::key_action_with_shifted(&[], b'^' as u64, Some(b'+' as u64), 5),
        Some("font-increase")
    );
    assert_eq!(
        actions::key_action_with_shifted(&[], b'^' as u64, Some(b'+' as u64), 5 | 2 | 16),
        Some("font-increase")
    );
    assert_eq!(
        actions::key_action_with_shifted(&[], b'^' as u64, Some(b'+' as u64), 4),
        None
    );
    assert_eq!(
        actions::key_action_with_shifted(&[], b'0' as u64, Some(b')' as u64), 5),
        Some("font-reset")
    );
    assert_eq!(
        actions::key_action_with_shifted(&[], b'=' as u64, Some(b'+' as u64), 13),
        Some("grow")
    );
}

#[test]
fn custom_shifted_and_base_bindings_override_new_zoom_defaults() {
    let mut bindings = vec![actions::parse("ctrl+shift+^ = none", false).unwrap()];
    assert_eq!(
        actions::key_action_with_shifted(&bindings, b'^' as u64, Some(b'+' as u64), 5),
        Some("none")
    );
    actions::set(
        &mut bindings,
        actions::parse("ctrl+shift+plus = palette", false).unwrap(),
    )
    .unwrap();
    assert_eq!(
        actions::key_action_with_shifted(&bindings, b'^' as u64, Some(b'+' as u64), 5),
        Some("palette")
    );
    actions::set(
        &mut bindings,
        actions::parse("ctrl+shift+plus = none", false).unwrap(),
    )
    .unwrap();
    actions::set(
        &mut bindings,
        actions::parse("ctrl+shift+^ = search", false).unwrap(),
    )
    .unwrap();
    assert_eq!(
        actions::key_action_with_shifted(&bindings, b'^' as u64, Some(b'+' as u64), 5),
        Some("none")
    );
    bindings.retain(|binding| binding.symbol != b'=' as u64);
    assert_eq!(
        actions::key_action_with_shifted(&bindings, b'^' as u64, Some(b'+' as u64), 5),
        Some("search")
    );
    actions::set(
        &mut bindings,
        actions::parse("ctrl+kp-add = font-decrease", false).unwrap(),
    )
    .unwrap();
    assert_eq!(
        actions::key_action(&bindings, 0xffab, 4),
        Some("font-decrease")
    );
    assert_eq!(
        actions::key_action(&bindings, b'+' as u64, 4),
        Some("font-increase")
    );
}

#[test]
fn tab_aliases_preserve_application_tab_and_respect_custom_remaps() {
    for locks in [0, 2 | 16] {
        assert_eq!(
            actions::key_action(&[], 0xff09, 4 | locks),
            Some("next-tab")
        );
        assert_eq!(
            actions::key_action(&[], 0xff09, 5 | locks),
            Some("previous-tab")
        );
        assert_eq!(
            actions::key_action(&[], 0xfe20, 5 | locks),
            Some("previous-tab")
        );
    }
    assert_eq!(actions::key_action(&[], 0xff09, 0), None);
    assert_eq!(actions::key_action(&[], 0xfe20, 1), None);
    assert_eq!(actions::key_action(&[], 0xff09, 4 | 8), None);
    let mut bindings = vec![actions::parse("ctrl+tab = none", false).unwrap()];
    assert_eq!(actions::key_action(&bindings, 0xff09, 4), Some("none"));
    assert_eq!(
        actions::shortcut(&bindings, "next-tab").as_deref(),
        Some("Ctrl+Shift+Pagedown")
    );
    actions::set(
        &mut bindings,
        actions::parse("ctrl+shift+tab = copy", false).unwrap(),
    )
    .unwrap();
    assert_eq!(actions::key_action(&bindings, 0xfe20, 5), Some("copy"));
    assert_eq!(
        actions::shortcut(&bindings, "previous-tab").as_deref(),
        Some("Ctrl+Shift+Pageup")
    );
}

#[test]
fn keypad_binding_names_round_trip_and_hints_follow_overrides() {
    for (key, symbol, action) in [
        ("kp-add", 0xffab, "font-increase"),
        ("kp-subtract", 0xffad, "font-decrease"),
        ("kp-0", 0xffb0, "font-reset"),
    ] {
        let binding = actions::parse(&format!("ctrl+{key} = {action}"), false).unwrap();
        assert_eq!(binding.symbol, symbol);
        assert_eq!(actions::parse(&binding.config(), false).unwrap(), binding);
        assert_eq!(
            actions::key_action(&[binding], symbol, 4 | 16),
            Some(action)
        );
    }
    let bindings = [
        actions::parse("ctrl+equal = none", false).unwrap(),
        actions::parse("ctrl+shift+equal = none", false).unwrap(),
    ];
    assert_eq!(
        actions::shortcut(&bindings, "font-increase").as_deref(),
        Some("Ctrl+Kp-add")
    );
}
