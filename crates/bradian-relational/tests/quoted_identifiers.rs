//! ADR 0181 — quoted identifier syntax `"name"` in the SQL lexer.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn open(dir: &std::path::Path) -> SqlEngine { SqlEngine::open(dir).unwrap() }

#[test]
fn quoted_identifier_in_create_table_name_is_accepted() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    db.execute(r#"CREATE DATABASE "t""#).unwrap();
    db.execute(r#"CREATE SCHEMA "t"."public""#).unwrap();
    db.execute(r#"CREATE TABLE "t"."public"."items" (id BIGINT NOT NULL, PRIMARY KEY(id))"#).unwrap();
    // Unquoted resolves to the same object (catalog folds to
    // lowercase).
    db.execute("INSERT INTO t.public.items(id) VALUES(1)").unwrap();
}

#[test]
fn quoted_identifier_can_hold_a_reserved_word_as_column_name() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    db.execute("CREATE DATABASE t").unwrap();
    db.execute("CREATE SCHEMA t.public").unwrap();
    // "select" is a reserved word — only addressable via the
    // quoted form.
    db.execute(r#"CREATE TABLE t.public.a (id BIGINT NOT NULL, "select" TEXT NOT NULL, PRIMARY KEY(id))"#).unwrap();
    db.execute(r#"INSERT INTO t.public.a(id, "select") VALUES(1, 'reserved')"#).unwrap();
    match db.execute(r#"SELECT "select" FROM t.public.a WHERE id=1"#).unwrap() {
        SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
            Value::String(s) => assert_eq!(s, "reserved"),
            other => panic!("expected String; got {other:?}"),
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn unquoted_and_quoted_names_resolve_to_the_same_catalog_object() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    db.execute("CREATE DATABASE t").unwrap();
    db.execute("CREATE SCHEMA t.public").unwrap();
    db.execute(r#"CREATE TABLE "T"."PUBLIC"."ITEMS" (id BIGINT NOT NULL, PRIMARY KEY(id))"#).unwrap();
    db.execute("INSERT INTO t.public.items(id) VALUES(1)").unwrap();
    match db.execute(r#"SELECT id FROM "t"."public"."items" WHERE id=1"#).unwrap() {
        SqlResultSet::Rows { rows, .. } => assert_eq!(rows.len(), 1),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn escaped_double_quote_inside_quoted_identifier_is_parsed() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    db.execute("CREATE DATABASE t").unwrap();
    db.execute("CREATE SCHEMA t.public").unwrap();
    // Column named exactly `has"quote` — unreachable unquoted.
    // The catalog identifier() still rejects non-alphanumeric
    // characters in names, so this should fail at the catalog
    // check — but the *lexer* must accept the token first.
    let err = db.execute(
        r#"CREATE TABLE t.public.a (id BIGINT NOT NULL, "has""quote" TEXT NOT NULL, PRIMARY KEY(id))"#
    ).unwrap_err();
    // Lexer accepted the token; catalog rejects the identifier
    // because it contains a non-alphanumeric byte.
    assert!(matches!(err, SqlError::Catalog(_)),
        "lexer should accept the escaped quote; catalog rejects content; got {err:?}");
}

#[test]
fn empty_quoted_identifier_is_rejected_at_lex() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    db.execute("CREATE DATABASE t").unwrap();
    db.execute("CREATE SCHEMA t.public").unwrap();
    let err = db.execute(
        r#"CREATE TABLE t.public."" (id BIGINT NOT NULL, PRIMARY KEY(id))"#
    ).unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)),
        "empty quoted identifier must fail at lex; got {err:?}");
}

#[test]
fn unterminated_quoted_identifier_is_rejected_at_lex() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    db.execute("CREATE DATABASE t").unwrap();
    db.execute("CREATE SCHEMA t.public").unwrap();
    let err = db.execute(
        r#"CREATE TABLE t.public."unterminated (id BIGINT NOT NULL, PRIMARY KEY(id))"#
    ).unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)),
        "unterminated quoted identifier must fail; got {err:?}");
}

#[test]
fn quoted_identifier_does_not_match_keyword_in_consume_word_path() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    db.execute("CREATE DATABASE t").unwrap();
    db.execute("CREATE SCHEMA t.public").unwrap();
    // Writing `"primary" key (id)` where PRIMARY is a keyword:
    // the quoted `"primary"` is NOT the keyword, so the parser
    // reads it as a column name — which errors as "expected
    // PRIMARY KEY".  This proves keywords don't bind to quoted
    // forms.
    let err = db.execute(
        r#"CREATE TABLE t.public.a (id BIGINT NOT NULL, "primary" key (id))"#
    ).unwrap_err();
    // Either a syntax error (expected type after "primary"
    // column name) or a resolution error — the key point is
    // the quoted form didn't match the `primary` keyword.
    assert!(
        matches!(err, SqlError::Syntax(_) | SqlError::Unsupported(_) | SqlError::Resolution(_)),
        "quoted 'primary' must not match the keyword; got {err:?}",
    );
}
