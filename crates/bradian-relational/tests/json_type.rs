//! ADR 0099 — JSON logical type smoke tests.

use bradian_core::{LogicalType, Value};

#[test]
fn json_logical_type_round_trips_through_value() {
    let payload = serde_json::json!({"name": "ou1", "level": 3, "children": [1,2,3]});
    let canonical = serde_json::to_string(&payload).unwrap();
    let v = Value::Json(canonical.clone());
    assert_eq!(v.logical_type(), Some(LogicalType::Json));
    match &v {
        Value::Json(s) => {
            let parsed: serde_json::Value = serde_json::from_str(s).unwrap();
            assert_eq!(parsed, payload);
        }
        other => panic!("expected Json, got {other:?}"),
    }
}

#[test]
fn json_values_hash_and_compare_consistently() {
    let a = Value::Json("{\"x\":1}".into());
    let b = Value::Json("{\"x\":1}".into());
    let c = Value::Json("{\"x\":2}".into());
    assert_eq!(a, b);
    assert_ne!(a, c);
    use std::collections::HashSet;
    let mut set = HashSet::new();
    set.insert(a.clone());
    assert!(set.contains(&b));
    assert!(!set.contains(&c));
}

#[test]
fn logical_json_defaults_to_utf8_physical() {
    assert_eq!(LogicalType::Json.default_physical(),
        bradian_core::PhysicalType::Utf8);
}

#[test]
fn json_sorts_after_period_tag() {
    // Value tags (see Ord impl): Period=8, Json=9.
    // Within tag equality, Json strings sort lexicographically.
    let per = Value::Period("202601".into());
    let jsn = Value::Json("{}".into());
    assert!(per < jsn, "Period tag < Json tag");
    let j1 = Value::Json("a".into());
    let j2 = Value::Json("b".into());
    assert!(j1 < j2);
}
