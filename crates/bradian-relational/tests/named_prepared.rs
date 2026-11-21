//! ADR 0183 — named parameters in prepared statements.

use std::collections::BTreeMap;

use bradian_relational::driver::{bind_literals, bind_named_literals, Driver, NoopDriver};
use bradian_relational::prepared::{scan_named_placeholders, PreparedError, PreparedStatement};
use bradian_relational::typed_table::Value;

fn named(entries: &[(&str, Value)]) -> BTreeMap<String, Value> {
    entries.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

#[test]
fn named_only_statement_validates_and_binds() {
    let stmt = PreparedStatement::with_named(
        "SELECT * FROM t WHERE id = :id AND tag = :tag",
        named(&[("id", Value::Int64(7)), ("tag", Value::String("ok".into()))]),
    );
    stmt.validate().unwrap();
    let bound = bind_named_literals(&stmt.sql, &stmt.named_params).unwrap();
    assert_eq!(bound, "SELECT * FROM t WHERE id = 7 AND tag = 'ok'");
}

#[test]
fn positional_and_named_may_coexist_in_one_template() {
    let stmt = PreparedStatement {
        sql: "SELECT $1 FROM t WHERE id = :id".into(),
        params: vec![Value::Int64(1)],
        named_params: named(&[("id", Value::Int64(42))]),
    };
    stmt.validate().unwrap();
    let pos = bind_literals(&stmt.sql, &stmt.params).unwrap();
    let both = bind_named_literals(&pos, &stmt.named_params).unwrap();
    assert_eq!(both, "SELECT 1 FROM t WHERE id = 42");
}

#[test]
fn missing_named_binding_fails_validate() {
    let stmt = PreparedStatement::with_named(
        "SELECT * FROM t WHERE id = :id AND tag = :tag",
        named(&[("id", Value::Int64(7))]),
    );
    let err = stmt.validate().unwrap_err();
    assert!(matches!(err, PreparedError::UnknownNamedParameter { ref name } if name == "tag"));
}

#[test]
fn unreferenced_named_binding_fails_validate() {
    let stmt = PreparedStatement::with_named(
        "SELECT * FROM t WHERE id = :id",
        named(&[("id", Value::Int64(7)), ("extra", Value::Int64(0))]),
    );
    let err = stmt.validate().unwrap_err();
    assert!(matches!(err, PreparedError::UnusedNamedParameter { ref name } if name == "extra"));
}

#[test]
fn invalid_named_identifier_is_rejected_at_validate() {
    let mut map = BTreeMap::new();
    map.insert("".to_string(), Value::Int64(1));
    let stmt = PreparedStatement { sql: "SELECT 1".into(), params: vec![], named_params: map };
    assert!(matches!(stmt.validate().unwrap_err(),
        PreparedError::InvalidNamedParameter { .. }));

    let mut map = BTreeMap::new();
    map.insert("1foo".to_string(), Value::Int64(1));
    let stmt = PreparedStatement { sql: "SELECT 1".into(), params: vec![], named_params: map };
    assert!(matches!(stmt.validate().unwrap_err(),
        PreparedError::InvalidNamedParameter { .. }));
}

#[test]
fn repeated_named_reference_binds_the_same_value() {
    let stmt = PreparedStatement::with_named(
        "SELECT :id, :id, :id",
        named(&[("id", Value::Int64(9))]),
    );
    stmt.validate().unwrap();
    let bound = bind_named_literals(&stmt.sql, &stmt.named_params).unwrap();
    assert_eq!(bound, "SELECT 9, 9, 9");
}

#[test]
fn scan_named_placeholders_returns_distinct_names() {
    let found = scan_named_placeholders("SELECT :a, :b, :a, :_c");
    let names: Vec<&String> = found.iter().collect();
    assert_eq!(names, vec![&"_c".to_string(), &"a".to_string(), &"b".to_string()]);
}

#[test]
fn colon_without_identifier_is_literal() {
    // `:5` (colon followed by digit) is not a named placeholder.
    // `::foo` has two colons; the first is literal and the second
    // starts an identifier `foo`.
    let found = scan_named_placeholders("WHERE t.x = :5 AND y = ::foo");
    assert_eq!(found.iter().cloned().collect::<Vec<_>>(), vec!["foo".to_string()]);
}

#[test]
fn serde_round_trip_preserves_named_params() {
    let stmt = PreparedStatement::with_named(
        "SELECT * FROM t WHERE id = :id",
        named(&[("id", Value::Int64(42))]),
    );
    let json = serde_json::to_string(&stmt).unwrap();
    let back: PreparedStatement = serde_json::from_str(&json).unwrap();
    assert_eq!(stmt, back);
    // Backward compat: an envelope without `named_params`
    // deserializes fine (serde default).
    let legacy = r#"{"sql":"SELECT 1","params":[]}"#;
    let legacy_stmt: PreparedStatement = serde_json::from_str(legacy).unwrap();
    assert!(legacy_stmt.named_params.is_empty());
}

#[test]
fn execute_prepared_binds_both_styles_through_driver_trait() {
    let driver = NoopDriver::new();
    let stmt = PreparedStatement {
        sql: "INSERT INTO t.public.a(id, name) VALUES($1, :name)".into(),
        params: vec![Value::Int64(1)],
        named_params: named(&[("name", Value::String("alice".into()))]),
    };
    // Even though NoopDriver returns NotWired, the binding
    // step runs first and the log records the final bound SQL.
    let _ = driver.execute_prepared(&stmt);
    let log = driver.log();
    assert_eq!(log, vec!["INSERT INTO t.public.a(id, name) VALUES(1, 'alice')".to_string()]);
}
