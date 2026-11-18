extern crate alloc;

use ztty::session_control::{SessionControl, valid_actor};

#[test]
fn initial_owner_has_exclusive_control() {
    let control = SessionControl::new("local".into());
    assert_eq!(control.owner(), Some("local"));
    assert!(control.owns("local"));
    assert!(!control.owns("mcp:first"));
    assert!(!control.owns("LOCAL"));
}

#[test]
fn reacquiring_current_owner_does_not_request_input_cancellation() {
    let mut control = SessionControl::new("mcp:first".into());
    // The caller cancels its queue and paste state only for a true result.
    for takeover in [false, true, false] {
        assert_eq!(control.acquire("mcp:first", takeover), Ok(false));
        assert_eq!(control.owner(), Some("mcp:first"));
    }
}

#[test]
fn rejected_takeover_keeps_the_original_owner() {
    let mut control = SessionControl::new("local".into());
    assert_eq!(
        control.acquire("mcp:first", false),
        Err("session already has an input owner; explicit takeover is required")
    );
    assert!(control.owns("local"));
    assert!(!control.owns("mcp:first"));
    assert_eq!(control.acquire("local", false), Ok(false));
}

#[test]
fn explicit_takeover_revokes_previous_owner_and_requests_cancellation_once() {
    let mut control = SessionControl::new("local".into());
    assert_eq!(control.acquire("mcp:first", true), Ok(true));
    assert!(!control.owns("local"));
    assert!(control.owns("mcp:first"));
    assert_eq!(control.acquire("mcp:first", true), Ok(false));
    assert_eq!(control.acquire("local", true), Ok(true));
    assert!(control.owns("local"));
    assert!(!control.owns("mcp:first"));
}

#[test]
fn release_by_another_actor_does_not_change_control() {
    let mut control = SessionControl::new("mcp:first".into());
    assert_eq!(
        control.release("local"),
        Err("only the input owner can release control")
    );
    assert_eq!(control.owner(), Some("mcp:first"));
    assert_eq!(control.acquire("mcp:first", false), Ok(false));
}

#[test]
fn released_control_denies_every_actor_until_reacquired() {
    let mut control = SessionControl::new("mcp:first".into());
    assert_eq!(control.release("mcp:first"), Ok(()));
    assert_eq!(control.owner(), None);
    for actor in ["local", "mcp:first", "mcp:second"] {
        assert!(!control.owns(actor));
        assert_eq!(
            control.release(actor),
            Err("only the input owner can release control")
        );
        assert_eq!(control.owner(), None);
    }
}

#[test]
fn an_unowned_session_can_be_acquired_without_forcing_takeover() {
    for next_owner in ["local", "mcp:first", "mcp:second"] {
        let mut control = SessionControl::new("mcp:first".into());
        control.release("mcp:first").unwrap();
        assert_eq!(control.acquire(next_owner, false), Ok(true));
        assert_eq!(control.owner(), Some(next_owner));
        assert_eq!(control.acquire(next_owner, false), Ok(false));
    }
}

#[test]
fn repeated_transfers_keep_one_owner_and_preserve_rejected_state() {
    let actors = ["local", "mcp:first", "mcp:second", "observer-3"];
    let mut control = SessionControl::new(actors[0].into());
    for round in 0..32 {
        let owner = actors[round % actors.len()];
        for actor in actors {
            assert_eq!(control.owns(actor), actor == owner);
            if actor != owner {
                assert!(control.acquire(actor, false).is_err());
                assert!(control.release(actor).is_err());
                assert_eq!(control.owner(), Some(owner));
            }
        }
        let next = actors[(round + 1) % actors.len()];
        assert_eq!(control.acquire(next, true), Ok(true));
    }
}

#[test]
fn actor_validation_retains_ascii_and_length_boundaries() {
    for actor in [
        "local",
        "mcp:1234567890abcdef",
        "A_Z-9.example:1",
        ".",
        ":",
        "_",
        "-",
    ] {
        assert!(valid_actor(actor), "{actor:?}");
    }
    assert!(valid_actor(&"a".repeat(128)));
    assert!(!valid_actor(&"a".repeat(129)));
    for actor in [
        "",
        "with space",
        "mcp/stdio",
        "line\nbreak",
        "tab\tbreak",
        "nul\0byte",
        "user@host",
        "中文",
        "é",
        "a\u{7f}",
        "a\r",
    ] {
        assert!(!valid_actor(actor), "{actor:?}");
    }
}
