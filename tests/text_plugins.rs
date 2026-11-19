use ztty::text_plugins::{MAX_PLUGINS, Registration, set};

#[test]
fn registration_is_bounded_normalized_and_does_not_require_loading() {
    let value = Registration::parse("Clean_JSON=/tmp/插件 with spaces.so").unwrap();
    assert_eq!(value.name, "clean_json");
    assert_eq!(value.path, "/tmp/插件 with spaces.so");
    assert_eq!(Registration::parse(&value.config()).unwrap(), value);
    for bad in [
        "",
        "name",
        "=x",
        "1name=/tmp/a",
        "na me=/tmp/a",
        "é=/tmp/a",
        "name=relative.so",
        "name=/tmp/a\n",
        "name=/tmp/a\0",
    ] {
        assert!(Registration::parse(bad).is_err(), "{bad:?}");
    }
    assert!(Registration::parse(&format!("{}=/tmp/a", "n".repeat(33))).is_err());
    assert!(Registration::parse(&format!("name=/{}", "a".repeat(4096))).is_err());
    assert!(Registration::parse("name=/tmp/a\nb").is_err());
}

#[test]
fn overrides_do_not_consume_slots_and_failed_entries_are_transactional() {
    let mut entries = Vec::new();
    for i in 0..MAX_PLUGINS {
        set(&mut entries, &format!("action{i}=/tmp/{i}.so")).unwrap();
    }
    set(&mut entries, "ACTION0=/tmp/replacement.so").unwrap();
    assert_eq!(entries.len(), MAX_PLUGINS);
    assert_eq!(entries[0].path, "/tmp/replacement.so");
    let before = entries.clone();
    assert!(set(&mut entries, "extra=/tmp/extra.so").is_err());
    assert!(set(&mut entries, "ACTION0=relative.so").is_err());
    assert_eq!(entries, before);
}
