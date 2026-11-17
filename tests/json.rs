extern crate alloc;
#[path = "../src/json.rs"]
mod json;
use json::{Value, object, parse, stringify};

#[test]
fn unicode_roundtrip_preserves_request_identifiers() {
    let input = br#"{"id":9007199254740993123456789,"text":"\ud83d\ude80 \u4f60\u597d\n\\\"","items":[null,true,false,1.2e-3]}"#;
    let value = parse(input).unwrap();
    assert_eq!(
        value.get("id"),
        Some(&Value::Number("9007199254740993123456789".into()))
    );
    assert_eq!(value.get("text").unwrap().as_str(), Some("🚀 你好\n\\\""));
    assert_eq!(parse(stringify(&value).as_bytes()).unwrap(), value);
}

#[test]
fn rejects_ambiguous_and_malformed_json() {
    for bad in [
        r#"{"a":1,"a":2}"#,
        r#""\ud800""#,
        r#""\udc00""#,
        r#""\ud800\u0041""#,
        "[1,]",
        r#"{"a":1,}"#,
        "01",
        "-",
        "1.",
        "1e",
        "NaN",
        "true false",
        "\"un\nescaped\"",
    ] {
        assert!(parse(bad.as_bytes()).is_err(), "{bad}");
    }
    assert!(parse(&[b'"', 0xff, b'"']).is_err());
    assert!(parse(("[".repeat(66) + &"]".repeat(66)).as_bytes()).is_err());
    assert!(parse(&vec![b' '; json::MAX_MESSAGE + 1]).is_err());
}

#[test]
fn values_build_and_read_without_loss() {
    let mut value = object([("n", Value::from(7u64)), ("b", true.into())]);
    value.set("name", "test".into());
    value.set("n", (-9i32).into());
    assert_eq!(value.get("n").unwrap().as_i64(), Some(-9));
    assert_eq!(value.get("n").unwrap().as_u64(), None);
    assert_eq!(value.get("b").unwrap().as_bool(), Some(true));
    assert_eq!(value.get("name").unwrap().as_str(), Some("test"));
    assert_eq!(Value::Array(vec![1u64.into()]).as_array().unwrap().len(), 1);
    assert_eq!(parse(stringify(&value).as_bytes()).unwrap(), value);
}
