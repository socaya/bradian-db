//! ADR 0175 — `CREATE TABLE ... COLLATE <name>` and ORDER BY usage.
//!
//! Covers:
//! - Parser accepts `COLLATE binary` and `COLLATE nocase` on Text / Varchar.
//! - Unknown `COLLATE <name>` returns `SqlError::Unsupported`.
//! - COLLATE on a non-text column surfaces as `SqlError::Catalog`
//!   (definition error from the catalog layer).
//! - ORDER BY uses the declared collation: nocase interleaves
//!   mixed-case strings; binary (default) sorts uppercase first.
//! - A column with no COLLATE clause keeps binary semantics.

use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::history_migration::migrate_shared_to_history;
use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

#[test]
fn create_table_accepts_collate_binary_and_nocase_on_text_columns() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.a (id BIGINT NOT NULL, label TEXT NOT NULL COLLATE binary, PRIMARY KEY(id))",
        "CREATE TABLE t.public.b (id BIGINT NOT NULL, label TEXT NOT NULL COLLATE nocase, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
}

#[test]
fn create_table_rejects_unknown_collate_name() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
    ] { db.execute(sql).unwrap(); }
    let err = db.execute(
        "CREATE TABLE t.public.a (id BIGINT NOT NULL, label TEXT NOT NULL COLLATE icu_root, PRIMARY KEY(id))"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)),
        "unknown COLLATE must be Unsupported; got {err:?}");
}

#[test]
fn create_table_rejects_collate_on_non_text_column() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
    ] { db.execute(sql).unwrap(); }
    // COLLATE on BIGINT must be rejected by the catalog
    // (central definition check — never silently dropped).
    let err = db.execute(
        "CREATE TABLE t.public.a (id BIGINT NOT NULL COLLATE nocase, PRIMARY KEY(id))"
    ).unwrap_err();
    // Catalog definition error surfaces as SqlError::Catalog.
    assert!(matches!(err, SqlError::Catalog(_)),
        "COLLATE on non-text must be a Catalog error; got {err:?}");
}

/// ORDER BY uses the declared collation. `nocase` interleaves
/// mixed-case strings naturally; `binary` (default) sorts
/// uppercase ASCII before lowercase.
#[test]
fn order_by_uses_nocase_collation_when_declared() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("src");
    let history = dir.path().join("hist");
    {
        let ddl = SqlEngine::open(&source).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.n (id BIGINT NOT NULL, label TEXT NOT NULL COLLATE nocase, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let dom = DomainSqlEngine::open(&source).unwrap();
        dom.register_catalog_table("t.public.n").unwrap();
        dom.with_session(|s| {
            s.execute("BEGIN")?;
            s.execute("INSERT INTO t.public.n(id, label) VALUES(1, 'alpha')")?;
            s.execute("INSERT INTO t.public.n(id, label) VALUES(2, 'Beta')")?;
            s.execute("INSERT INTO t.public.n(id, label) VALUES(3, 'carrot')")?;
            s.execute("INSERT INTO t.public.n(id, label) VALUES(4, 'DELTA')")?;
            s.execute("COMMIT")?;
            Ok(())
        }).unwrap();
    }
    migrate_shared_to_history(&source, &history).unwrap();
    let db = HistorySqlEngine::open(&history).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        let r = s.execute("SELECT label FROM t.public.n ORDER BY label")?;
        match r {
            SqlResultSet::Rows { rows, .. } => {
                let got: Vec<String> = rows.iter().map(|r| match &r[0] {
                    Value::String(s) => s.clone(),
                    _ => String::new(),
                }).collect();
                // nocase ordering interleaves mixed case alphabetically.
                assert_eq!(got, vec!["alpha", "Beta", "carrot", "DELTA"]);
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn order_by_default_binary_collation_sorts_uppercase_before_lowercase() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("src");
    let history = dir.path().join("hist");
    {
        let ddl = SqlEngine::open(&source).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.b (id BIGINT NOT NULL, label TEXT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let dom = DomainSqlEngine::open(&source).unwrap();
        dom.register_catalog_table("t.public.b").unwrap();
        dom.with_session(|s| {
            s.execute("BEGIN")?;
            s.execute("INSERT INTO t.public.b(id, label) VALUES(1, 'alpha')")?;
            s.execute("INSERT INTO t.public.b(id, label) VALUES(2, 'Beta')")?;
            s.execute("INSERT INTO t.public.b(id, label) VALUES(3, 'carrot')")?;
            s.execute("INSERT INTO t.public.b(id, label) VALUES(4, 'DELTA')")?;
            s.execute("COMMIT")?;
            Ok(())
        }).unwrap();
    }
    migrate_shared_to_history(&source, &history).unwrap();
    let db = HistorySqlEngine::open(&history).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        let r = s.execute("SELECT label FROM t.public.b ORDER BY label")?;
        match r {
            SqlResultSet::Rows { rows, .. } => {
                let got: Vec<String> = rows.iter().map(|r| match &r[0] {
                    Value::String(s) => s.clone(),
                    _ => String::new(),
                }).collect();
                // Binary (ASCII) sort places uppercase ahead of lowercase.
                assert_eq!(got, vec!["Beta", "DELTA", "alpha", "carrot"]);
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn collate_binary_is_explicit_default_equivalent() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("src");
    let history = dir.path().join("hist");
    {
        let ddl = SqlEngine::open(&source).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.x (id BIGINT NOT NULL, label TEXT NOT NULL COLLATE binary, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let dom = DomainSqlEngine::open(&source).unwrap();
        dom.register_catalog_table("t.public.x").unwrap();
        dom.with_session(|s| {
            s.execute("BEGIN")?;
            for (id, label) in [(1, "aa"), (2, "AA"), (3, "ab"), (4, "AB")] {
                s.execute(&format!(
                    "INSERT INTO t.public.x(id, label) VALUES({id}, '{label}')"))?;
            }
            s.execute("COMMIT")?;
            Ok(())
        }).unwrap();
    }
    migrate_shared_to_history(&source, &history).unwrap();
    let db = HistorySqlEngine::open(&history).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        let r = s.execute("SELECT label FROM t.public.x ORDER BY label")?;
        match r {
            SqlResultSet::Rows { rows, .. } => {
                let got: Vec<String> = rows.iter().map(|r| match &r[0] {
                    Value::String(s) => s.clone(),
                    _ => String::new(),
                }).collect();
                assert_eq!(got, vec!["AA", "AB", "aa", "ab"]);
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}
