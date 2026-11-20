extern crate alloc;

#[path = "../src/backend.rs"]
mod backend;

use backend::{Backend, Preference, Requirements, initialize};

fn standalone() -> Requirements {
    Requirements {
        workspace: false,
        native_compatible: true,
        native_compiled: true,
    }
}

#[test]
fn default_auto_keeps_a_concrete_legacy_wire_value() {
    assert_eq!(Preference::default(), Preference::Auto);
    assert_eq!(Preference::parse(b"auto"), Ok(Preference::Auto));
    assert_eq!(Preference::parse(b"x11"), Ok(Preference::X11));
    assert_eq!(Preference::parse(b"wayland"), Ok(Preference::Wayland));
    assert!(Preference::parse(b"other").is_err());
    assert_eq!(Preference::Auto.legacy_backend().name(), "x11");
    assert_eq!(Preference::Wayland.legacy_backend().name(), "wayland");
}

#[test]
fn successful_x11_initialization_does_not_touch_native_wayland() {
    let mut attempts = Vec::new();
    let window = initialize(Preference::Auto, standalone(), |backend| {
        attempts.push(backend);
        Ok(17)
    });
    assert_eq!(window, Ok(17));
    assert_eq!(attempts, [Backend::X11]);
}

#[test]
fn auto_fallback_covers_full_view_initialization_failure() {
    let mut attempts = Vec::new();
    let window = initialize(Preference::Auto, standalone(), |backend| {
        attempts.push(backend);
        match backend {
            Backend::X11 => Err("renderer failed after connection opened".into()),
            Backend::Wayland => Ok(23),
        }
    });
    assert_eq!(window, Ok(23));
    assert_eq!(attempts, [Backend::X11, Backend::Wayland]);
}

#[test]
fn both_initialization_errors_remain_visible_and_no_attempt_is_repeated() {
    let mut attempts = Vec::new();
    let error = initialize::<()>(Preference::Auto, standalone(), |backend| {
        attempts.push(backend);
        Err(match backend {
            Backend::X11 => "bad X11 display".into(),
            Backend::Wayland => "bad Wayland endpoint".into(),
        })
    })
    .unwrap_err();
    assert!(error.contains("bad X11 display"));
    assert!(error.contains("bad Wayland endpoint"));
    assert_eq!(attempts, [Backend::X11, Backend::Wayland]);
}

#[test]
fn explicit_choices_do_not_fallback() {
    for (preference, expected) in [
        (Preference::X11, Backend::X11),
        (Preference::Wayland, Backend::Wayland),
    ] {
        let mut attempts = Vec::new();
        let error = initialize::<()>(preference, standalone(), |backend| {
            attempts.push(backend);
            Err("view failed".into())
        });
        assert_eq!(error, Err("view failed".into()));
        assert_eq!(attempts, [expected]);
    }
}

#[test]
fn workspace_auto_never_opens_wayland_after_x11_failure() {
    let mut requirements = standalone();
    requirements.workspace = true;
    let mut attempts = Vec::new();
    let error = initialize::<()>(Preference::Auto, requirements, |backend| {
        attempts.push(backend);
        Err("display unavailable".into())
    })
    .unwrap_err();
    assert!(error.contains("tabs and splits require X11"));
    assert_eq!(attempts, [Backend::X11]);
}

#[test]
fn explicit_wayland_workspace_fails_before_any_view_is_created() {
    let mut requirements = standalone();
    requirements.workspace = true;
    let error = initialize::<()>(Preference::Wayland, requirements, |_| {
        panic!("workspace must reject Wayland before initialization")
    });
    assert!(error.unwrap_err().contains("Wayland workspaces"));
}

#[test]
fn auto_preserves_reload_for_existing_non_native_sessions() {
    let mut requirements = standalone();
    requirements.native_compatible = false;
    let mut attempts = Vec::new();
    let error = initialize::<()>(Preference::Auto, requirements, |backend| {
        attempts.push(backend);
        Err("display unavailable".into())
    })
    .unwrap_err();
    assert!(error.contains("disable runtime reload for existing sessions"));
    assert_eq!(attempts, [Backend::X11]);
}

#[test]
fn explicit_wayland_retains_its_existing_mixed_pool_semantics() {
    let mut requirements = standalone();
    requirements.native_compatible = false;
    let mut attempts = Vec::new();
    let window = initialize(Preference::Wayland, requirements, |backend| {
        attempts.push(backend);
        Ok(31)
    });
    assert_eq!(window, Ok(31));
    assert_eq!(attempts, [Backend::Wayland]);
}

#[test]
fn auto_explains_an_uncompiled_optional_backend_without_trying_it() {
    let mut requirements = standalone();
    requirements.native_compiled = false;
    let mut attempts = Vec::new();
    let error = initialize::<()>(Preference::Auto, requirements, |backend| {
        attempts.push(backend);
        Err("display unavailable".into())
    })
    .unwrap_err();
    assert!(error.contains("build with --features wayland"));
    assert_eq!(attempts, [Backend::X11]);
}

#[test]
fn detached_native_auto_restores_its_known_backend_without_cross_backend_retry() {
    let mut requirements = standalone();
    requirements.native_compatible = false;
    let preference = Preference::Auto.for_attachment(Some(Backend::Wayland));
    let mut attempts = Vec::new();
    let window = initialize(preference, requirements, |backend| {
        attempts.push(backend);
        Ok(41)
    });
    assert_eq!(window, Ok(41));
    assert_eq!(attempts, [Backend::Wayland]);
}

#[test]
fn detached_x11_auto_does_not_migrate_after_restore_failure() {
    let preference = Preference::Auto.for_attachment(Some(Backend::X11));
    let mut attempts = Vec::new();
    let error = initialize::<()>(preference, standalone(), |backend| {
        attempts.push(backend);
        Err("previous display unavailable".into())
    });
    assert_eq!(error, Err("previous display unavailable".into()));
    assert_eq!(attempts, [Backend::X11]);
}

#[test]
fn attachment_provenance_does_not_override_explicit_choices() {
    assert_eq!(
        Preference::X11.for_attachment(Some(Backend::Wayland)),
        Preference::X11
    );
    assert_eq!(
        Preference::Wayland.for_attachment(Some(Backend::X11)),
        Preference::Wayland
    );
    assert_eq!(Preference::Auto.for_attachment(None), Preference::Auto);
}

#[test]
fn checkpoint_provenance_preserves_detached_native_and_accepts_legacy_snapshots() {
    assert_eq!(
        Backend::from_checkpoint(Some("wayland"), None),
        Ok(Some(Backend::Wayland))
    );
    assert_eq!(
        Backend::from_checkpoint(None, Some("x11")),
        Ok(Some(Backend::X11))
    );
    assert_eq!(Backend::from_checkpoint(None, None), Ok(None));
    assert_eq!(
        Backend::from_checkpoint(Some("x11"), Some("x11")),
        Ok(Some(Backend::X11))
    );
}

#[test]
fn checkpoint_provenance_rejects_unknown_or_conflicting_backends() {
    assert!(Backend::from_checkpoint(Some("auto"), None).is_err());
    assert!(Backend::from_checkpoint(Some("unknown"), None).is_err());
    assert!(Backend::from_checkpoint(Some("wayland"), Some("x11")).is_err());
}
