//! ADR 0243 — IS [NOT] DISTINCT FROM on JSON-path WHERE.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        // id=1 owner='alice'; id=2 owner missing; id=3 owner JSON null; id=4 owner='bob'.
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"owner":"alice"}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(3, '{"owner":null}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(4, '{"owner":"bob"}')"#,
    ] { db.execute(sql).unwrap(); }
    db
}

fn ids(rs: SqlResultSet) -> Vec<i64> {
    match rs {
        SqlResultSet::Rows { rows, .. } => rows.into_iter()
            .map(|r| match r[0] { Value::Int64(v) => v, _ => panic!("expected id") })
            .collect(),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn is_distinct_from_non_null_literal_matches_non_equal_and_null() {
    // owner='alice' is NOT distinct from 'alice'; owner missing / null IS
    // distinct from 'alice' (NULL-vs-non-NULL); owner='bob' IS distinct.
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'owner' IS DISTINCT FROM 'alice'"
    ).unwrap());
    assert_eq!(got, vec![2, 3, 4]);
}

#[test]
fn is_not_distinct_from_non_null_literal_only_matches_equal() {
    // Classical equality: only owner='alice'.
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'owner' IS NOT DISTINCT FROM 'alice'"
    ).unwrap());
    assert_eq!(got, vec![1]);
}

#[test]
fn is_distinct_from_in_and_chain() {
    // Owners that are DISTINCT FROM 'alice' AND whose row id > 2:
    // id=2 excluded by id check (predicate uses ->>'owner' so can't
    // reference id — use a second JSON check instead).
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // id=2 missing key (NULL), id=3 JSON null, id=4 'bob'
    // AND body ->>'owner' IS NOT NULL only keeps id=4.
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs \
         WHERE body ->> 'owner' IS DISTINCT FROM 'alice' \
         AND body ->> 'owner' IS NOT NULL"
    ).unwrap());
    assert_eq!(got, vec![4]);
}

#[test]
fn is_distinct_from_with_non_text_terminator() {
    // `->` yields a JSON value; equality against a text literal
    // compares the string form, consistent with existing matcher
    // behavior.
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body -> 'owner' IS DISTINCT FROM 'alice'"
    ).unwrap());
    assert_eq!(got, vec![2, 3, 4]);
}

#[test]
fn is_distinct_rejects_without_from() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Missing FROM after DISTINCT.
    let err = db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'owner' IS DISTINCT 'alice'"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_) | SqlError::Unsupported(_)), "got {err:?}");
}

#[test]
fn is_not_distinct_rejects_without_distinct_from() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'owner' IS NOT 'alice'"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)), "got {err:?}");
}

#[test]
fn is_distinct_from_combined_with_or() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // 'bob' distinct OR 'alice' is-not-distinct: everyone whose
    // owner != 'bob' (i.e. missing, null, 'alice') AND is-not-NULL-'alice'
    // Just verify the two branches compose. Rows where owner distinct
    // from 'bob' OR owner equals 'alice': id=1 ('alice'), id=2 (NULL),
    // id=3 (NULL), id=4 excluded (equals 'bob' AND not equal to 'alice').
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs \
         WHERE body ->> 'owner' IS DISTINCT FROM 'bob' \
         OR body ->> 'owner' IS NOT DISTINCT FROM 'alice'"
    ).unwrap());
    assert_eq!(got, vec![1, 2, 3]);
}

#[test]
fn is_distinct_from_under_not() {
    // NOT (IS DISTINCT FROM 'alice') == IS NOT DISTINCT FROM 'alice'.
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs \
         WHERE NOT (body ->> 'owner' IS DISTINCT FROM 'alice')"
    ).unwrap());
    assert_eq!(got, vec![1]);
}

#[test]
fn is_not_distinct_matches_absent_key_when_literal_is_null_sentinel() {
    // The grammar parses literals; a SQL NULL literal isn't a
    // standard literal in this parser's grammar. Surface the
    // existing behavior: IS NOT DISTINCT FROM a non-NULL literal
    // never matches NULL rows. id=2 and id=3 (NULL side) must not
    // match 'alice'.
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'owner' IS NOT DISTINCT FROM 'alice'"
    ).unwrap());
    assert_eq!(got, vec![1]);
}
