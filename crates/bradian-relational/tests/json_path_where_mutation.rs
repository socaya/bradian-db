//! ADR 0199 — JSON path predicates in UPDATE/DELETE WHERE.

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::history_migration::migrate_shared_to_history;
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::mvcc_sql::MvccSqlEngine;
use bradian_relational::sql::{SqlEngine, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed_sql_engine(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, label TEXT NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.docs(id, body, label) VALUES(1, '{"tier":1}', 'a')"#,
        r#"INSERT INTO t.public.docs(id, body, label) VALUES(2, '{"tier":2}', 'b')"#,
        r#"INSERT INTO t.public.docs(id, body, label) VALUES(3, '{"tier":3}', 'c')"#,
        r#"INSERT INTO t.public.docs(id, body, label) VALUES(4, '{"tier":4}', 'd')"#,
    ] { db.execute(sql).unwrap(); }
    db
}

fn labels(db: &SqlEngine) -> Vec<String> {
    match db.execute("SELECT label FROM t.public.docs").unwrap() {
        SqlResultSet::Rows { rows, .. } => rows.into_iter().map(|r| match r[0] {
            Value::String(ref s) => s.clone(),
            _ => panic!("expected String"),
        }).collect(),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn update_with_json_path_where_affects_matching_rows() {
    let dir = tempdir().unwrap();
    let db = seed_sql_engine(dir.path());
    let affected = match db.execute(
        "UPDATE t.public.docs SET label='X' WHERE body ->> 'tier' > '2'"
    ).unwrap() {
        SqlResultSet::Command { affected } => affected,
        other => panic!("expected Command; got {other:?}"),
    };
    // Lexicographic '>' on text: '2', '3', '4' all > '2' as
    // single-char strings → matches ids 3 and 4.
    assert_eq!(affected, 2);
    let mut got = labels(&db);
    got.sort();
    assert_eq!(got, vec!["X".to_string(), "X".to_string(), "a".to_string(), "b".to_string()]);
}

#[test]
fn update_with_json_path_equality_matches_single_row() {
    let dir = tempdir().unwrap();
    let db = seed_sql_engine(dir.path());
    match db.execute(
        "UPDATE t.public.docs SET label='only' WHERE body ->> 'tier' = '3'"
    ).unwrap() {
        SqlResultSet::Command { affected } => assert_eq!(affected, 1),
        other => panic!("expected Command; got {other:?}"),
    }
    match db.execute("SELECT label FROM t.public.docs WHERE id=3").unwrap() {
        SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
            Value::String(s) => assert_eq!(s, "only"),
            other => panic!("expected String; got {other:?}"),
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn update_with_json_path_where_no_match_returns_zero() {
    let dir = tempdir().unwrap();
    let db = seed_sql_engine(dir.path());
    match db.execute(
        "UPDATE t.public.docs SET label='nope' WHERE body ->> 'tier' = '99'"
    ).unwrap() {
        SqlResultSet::Command { affected } => assert_eq!(affected, 0),
        other => panic!("expected Command; got {other:?}"),
    }
}

#[test]
fn update_with_pk_equality_still_works_after_refactor() {
    let dir = tempdir().unwrap();
    let db = seed_sql_engine(dir.path());
    match db.execute("UPDATE t.public.docs SET label='X' WHERE id=2").unwrap() {
        SqlResultSet::Command { affected } => assert_eq!(affected, 1),
        other => panic!("expected Command; got {other:?}"),
    }
}

#[test]
fn update_with_json_path_rejects_pk_column_assignment() {
    let dir = tempdir().unwrap();
    let db = seed_sql_engine(dir.path());
    // PK UPDATE rejected regardless of WHERE form.
    let err = db.execute(
        "UPDATE t.public.docs SET id=99 WHERE body ->> 'tier' = '1'"
    ).unwrap_err();
    assert!(matches!(err, bradian_relational::sql::SqlError::Unsupported(_)));
}

#[test]
fn delete_with_json_path_where_removes_matching_rows() {
    let dir = tempdir().unwrap();
    let db = seed_sql_engine(dir.path());
    let affected = match db.execute(
        "DELETE FROM t.public.docs WHERE body ->> 'tier' < '3'"
    ).unwrap() {
        SqlResultSet::Command { affected } => affected,
        other => panic!("expected Command; got {other:?}"),
    };
    assert_eq!(affected, 2);
    let mut got = labels(&db);
    got.sort();
    assert_eq!(got, vec!["c".to_string(), "d".to_string()]);
}

#[test]
fn delete_with_json_path_equality() {
    let dir = tempdir().unwrap();
    let db = seed_sql_engine(dir.path());
    match db.execute(
        "DELETE FROM t.public.docs WHERE body ->> 'tier' = '2'"
    ).unwrap() {
        SqlResultSet::Command { affected } => assert_eq!(affected, 1),
        other => panic!("expected Command; got {other:?}"),
    }
}

#[test]
fn delete_pk_equality_still_works() {
    let dir = tempdir().unwrap();
    let db = seed_sql_engine(dir.path());
    match db.execute("DELETE FROM t.public.docs WHERE id=2").unwrap() {
        SqlResultSet::Command { affected } => assert_eq!(affected, 1),
        other => panic!("expected Command; got {other:?}"),
    }
}

#[test]
fn update_json_path_set_mutation_also_supported() {
    // Combine ADR 0192 SET JSON path with ADR 0199 WHERE
    // JSON path: mutate body->'tier' on all rows where
    // label matches a path condition.
    let dir = tempdir().unwrap();
    let db = seed_sql_engine(dir.path());
    db.execute(
        "UPDATE t.public.docs SET body -> 'tier' = 99 WHERE body ->> 'tier' = '2'"
    ).unwrap();
    match db.execute("SELECT body FROM t.public.docs WHERE id=2").unwrap() {
        SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
            Value::Json(s) => {
                let parsed: serde_json::Value = serde_json::from_str(s).unwrap();
                assert_eq!(parsed["tier"], 99);
            }
            other => panic!("expected Json; got {other:?}"),
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

// ---- Cross-engine sanity ----

#[test]
fn domain_update_with_json_path_where() {
    let dir = tempdir().unwrap();
    {
        let ddl = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, label TEXT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    let dom = DomainSqlEngine::open(dir.path()).unwrap();
    dom.register_catalog_table("t.public.docs").unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute(r#"INSERT INTO t.public.docs(id, body, label) VALUES(1, '{"k":"a"}', 'x')"#)?;
        s.execute(r#"INSERT INTO t.public.docs(id, body, label) VALUES(2, '{"k":"b"}', 'y')"#)?;
        s.execute("UPDATE t.public.docs SET label='Z' WHERE body ->> 'k' = 'b'")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT label FROM t.public.docs WHERE id=2")? {
            SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
                Value::String(v) => assert_eq!(v, "Z"),
                other => panic!("expected String; got {other:?}"),
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn history_update_with_json_path_where() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("src");
    let history = dir.path().join("hist");
    {
        let ddl = SqlEngine::open(&source).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.docs (id BIGINT NOT NULL, amount BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let dom = DomainSqlEngine::open(&source).unwrap();
        dom.register_catalog_table("t.public.docs").unwrap();
        dom.with_session(|s| {
            s.execute("BEGIN")?;
            s.execute(r#"INSERT INTO t.public.docs(id, amount, body) VALUES(1, 10, '{"n":"small"}')"#)?;
            s.execute(r#"INSERT INTO t.public.docs(id, amount, body) VALUES(2, 20, '{"n":"big"}')"#)?;
            s.execute("COMMIT")?;
            Ok(())
        }).unwrap();
    }
    migrate_shared_to_history(&source, &history).unwrap();
    let db = HistorySqlEngine::open(&history).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        // Expression RHS + JSON path WHERE in the same stmt.
        s.execute("UPDATE t.public.docs SET amount = amount + 5 WHERE body ->> 'n' = 'big'")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT amount FROM t.public.docs WHERE id=2")? {
            SqlResultSet::Rows { rows, .. } => assert_eq!(rows[0][0], Value::Int64(25)),
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn mvcc_delete_with_json_path_where() {
    let dir = tempdir().unwrap();
    {
        let ddl = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let dom = DomainSqlEngine::open(dir.path()).unwrap();
        dom.register_catalog_table("t.public.docs").unwrap();
    }
    let engine = MvccSqlEngine::open(dir.path()).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"k":"keep"}')"#)?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"k":"drop"}')"#)?;
        s.execute("DELETE FROM t.public.docs WHERE body ->> 'k' = 'drop'")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.docs")? {
            SqlResultSet::Rows { rows, .. } => {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0][0], Value::Int64(1));
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}
