//! ADR 0145 — engine-level integration of BudgetTracker
//! into the DomainSqlEngine SELECT path.

use std::time::Duration;

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::query_budget::{BudgetExceeded, QueryBudget};
use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use tempfile::tempdir;

fn setup(root: &std::path::Path) {
    let ddl = SqlEngine::open(root).unwrap();
    for stmt in [
        "CREATE DATABASE app",
        "CREATE SCHEMA app.public",
        "CREATE TABLE app.public.items (id BIGINT NOT NULL, name TEXT NOT NULL, PRIMARY KEY(id))",
    ] {
        ddl.execute(stmt).unwrap();
    }
}

fn open_registered(root: &std::path::Path) -> DomainSqlEngine {
    let db = DomainSqlEngine::open(root).unwrap();
    db.register_catalog_table("app.public.items").unwrap();
    db
}

fn seed(db: &DomainSqlEngine, rows: i64) {
    db.with_session(|s| {
        s.execute("BEGIN")?;
        for i in 0..rows {
            s.execute(&format!(
                "INSERT INTO app.public.items (id, name) VALUES ({i}, 'row{i}')"
            ))?;
        }
        s.execute("COMMIT")
    }).unwrap();
}

#[test]
fn select_within_budget_succeeds() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let db = open_registered(dir.path());
    seed(&db, 3);
    db.with_session_budget(QueryBudget {
        max_rows_scanned: Some(100),
        max_duration: Some(Duration::from_secs(30)),
        ..QueryBudget::unbounded()
    }, |s| {
        s.execute("BEGIN")?;
        let rs = s.execute("SELECT id FROM app.public.items")?;
        assert!(matches!(rs, SqlResultSet::Rows { rows, .. } if rows.len() == 3));
        s.execute("COMMIT")
    }).unwrap();
}

#[test]
fn select_over_row_budget_errors_with_sqlerror_budget() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let db = open_registered(dir.path());
    seed(&db, 5);
    let err = db.with_session_budget(QueryBudget {
        max_rows_scanned: Some(2),
        ..QueryBudget::unbounded()
    }, |s| {
        s.execute("BEGIN")?;
        s.execute("SELECT id FROM app.public.items")
    }).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Rows { .. })),
        "expected Budget::Rows, got {err:?}");
}

#[test]
fn select_over_deadline_errors_with_sqlerror_budget() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let db = open_registered(dir.path());
    seed(&db, 1);
    let err = db.with_session_budget(QueryBudget {
        max_duration: Some(Duration::from_millis(1)),
        ..QueryBudget::unbounded()
    }, |s| {
        s.execute("BEGIN")?;
        std::thread::sleep(Duration::from_millis(50));
        s.execute("SELECT id FROM app.public.items")
    }).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Duration { .. })),
        "expected Budget::Duration, got {err:?}");
}

#[test]
fn unbounded_budget_never_errors() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let db = open_registered(dir.path());
    seed(&db, 2);
    db.with_session_budget(QueryBudget::unbounded(), |s| {
        s.execute("BEGIN")?;
        for _ in 0..3 {
            s.execute("SELECT id FROM app.public.items")?;
        }
        s.execute("COMMIT")
    }).unwrap();
}

#[test]
fn budget_accumulates_across_statements_in_one_session() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let db = open_registered(dir.path());
    seed(&db, 2);
    let err = db.with_session_budget(QueryBudget {
        max_rows_scanned: Some(3),  // 2 + 2 = 4 > 3
        ..QueryBudget::unbounded()
    }, |s| {
        s.execute("BEGIN")?;
        s.execute("SELECT id FROM app.public.items")?;   // +2
        s.execute("SELECT id FROM app.public.items")     // +2 → breach
    }).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Rows { .. })),
        "expected Budget::Rows, got {err:?}");
}

#[test]
fn existing_with_session_path_unchanged() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let db = open_registered(dir.path());
    seed(&db, 1);
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("SELECT id FROM app.public.items")?;
        s.execute("COMMIT")
    }).unwrap();
}

#[test]
fn budget_does_not_charge_inserts() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let db = open_registered(dir.path());
    db.with_session_budget(QueryBudget {
        max_rows_scanned: Some(0),  // no scan allowed
        ..QueryBudget::unbounded()
    }, |s| {
        s.execute("BEGIN")?;
        // INSERT path doesn't charge; still succeeds under
        // 0-row budget.
        let rs = s.execute("INSERT INTO app.public.items (id, name) VALUES (1, 'x')")?;
        assert!(matches!(rs, SqlResultSet::Command { affected: 1 }));
        s.execute("COMMIT")
    }).unwrap();
}
