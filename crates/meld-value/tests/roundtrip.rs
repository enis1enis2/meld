use meld_value::{SourceStyle, Value};
use std::collections::BTreeMap;

#[test]
fn scalar_roundtrip() {
    let cases = vec![
        Value::Null,
        Value::Bool(true),
        Value::int(42),
        Value::String("héllo".to_string()),
        Value::Bytes(vec![0, 1, 2, 253, 254, 255]),
        Value::Float(1.5),
        Value::Float(f64::NAN),
    ];
    for v in cases {
        let j = v.to_json();
        let back = Value::from_json(&j).unwrap();
        // IEEE-754 NaN is not equal to itself; compare the bit class instead.
        match (&v, &back) {
            (Value::Float(a), Value::Float(b)) if a.is_nan() && b.is_nan() => {}
            _ => assert_eq!(back, v, "json: {j}"),
        }
    }
}

#[test]
fn composite_roundtrip() {
    let mut rec = BTreeMap::new();
    rec.insert("a".to_string(), Value::int(7));
    rec.insert("b".to_string(), Value::List(vec![Value::Bool(false), Value::Null]));
    let v = Value::Record(rec);
    let j = v.to_json();
    assert_eq!(Value::from_json(&j).unwrap(), v);
}

#[test]
fn bigint_does_not_round_trip_through_f64() {
    let v = Value::Int("123456789012345678901234567890".to_string());
    let j = v.to_json();
    assert!(j.is_string());
    assert_eq!(Value::from_json(&j).unwrap(), v);
}

#[test]
fn result_and_handle_tags() {
    let ok = Value::ResultOk(Box::new(Value::String("x".to_string())));
    assert_eq!(Value::from_json(&ok.to_json()).unwrap(), ok);
    let h = Value::Handle {
        id: "h7".to_string(),
        resource: "Db".to_string(),
        borrow: true,
    };
    assert_eq!(Value::from_json(&h.to_json()).unwrap(), h);
}

#[test]
fn checked_narrowing() {
    assert_eq!(Value::int(9).as_i64_checked().unwrap(), 9);
    let big = Value::Int("99999999999999999999999999".to_string());
    assert!(big.as_i64_checked().is_err());
}

#[test]
fn values_render_as_pack_style_source_literals() {
    let value = Value::List(vec![
        Value::Null,
        Value::Bool(true),
        Value::int(5),
        Value::String("a'b\n".to_string()),
    ]);
    assert_eq!(
        value.to_source_literal(SourceStyle::Python).unwrap(),
        "[None, True, 5, 'a\\'b\\n']"
    );
    assert_eq!(
        value.to_source_literal(SourceStyle::JavaScript).unwrap(),
        "[null, true, 5, \"a'b\\n\"]"
    );
    assert_eq!(
        value.to_source_literal(SourceStyle::Lua).unwrap(),
        "{nil, true, 5, \"a'b\\n\"}"
    );
    assert_eq!(SourceStyle::from_id("py"), Some(SourceStyle::Python));
}
