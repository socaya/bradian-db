//! ADR 0146 — engine-level integration of BudgetTracker
//! into the MvccSqlEngine SELECT path.

use std::time::Duration;

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::mvcc_sql::MvccSqlEngine;
use bradian_relational::query_budget::{BudgetExceeded, QueryBudget};
use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use tempfile::tempdir;

fn setup(root: &std::path::Path) {
    {
        let ddl = SqlEngine::open(root).unwrap();
        for sql in [
            "CREATE DATABASE app",
            "CREATE SCHEMA app.public",
            "CREATE TABLE app.public.items (id BIGINT NOT NULL, name TEXT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let domain = DomainSqlEngine::open(root).unwrap();
        domain.register_catalog_table("app.public.items").unwrap();
    }
}

fn seed(engine: &MvccSqlEngine, rows: i64) {
    engine.with_session(|s| {
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
    let eng = MvccSqlEngine::open(dir.path()).unwrap();
    seed(&eng, 3);
    eng.with_session_budget(QueryBudget {
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
    let eng = MvccSqlEngine::open(dir.path()).unwrap();
    seed(&eng, 5);
    let err = eng.with_session_budget(QueryBudget {
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
    let eng = MvccSqlEngine::open(dir.path()).unwrap();
    seed(&eng, 1);
    let err = eng.with_session_budget(QueryBudget {
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
    let eng = MvccSqlEngine::open(dir.path()).unwrap();
    seed(&eng, 2);
    eng.with_session_budget(QueryBudget::unbounded(), |s| {
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
    let eng = MvccSqlEngine::open(dir.path()).unwrap();
    seed(&eng, 2);
    let err = eng.with_session_budget(QueryBudget {
        max_rows_scanned: Some(3),  // 2 + 2 = 4 > 3
        ..QueryBudget::unbounded()
    }, |s| {
        s.execute("BEGIN")?;
        s.execute("SELECT id FROM app.public.items")?;
        s.execute("SELECT id FROM app.public.items")
    }).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Rows { .. })),
        "expected Budget::Rows, got {err:?}");
}

#[test]
fn existing_with_session_path_unchanged() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let eng = MvccSqlEngine::open(dir.path()).unwrap();
    seed(&eng, 1);
    eng.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("SELECT id FROM app.public.items")?;
        s.execute("COMMIT")
    }).unwrap();
}

#[test]
fn iteration_cap_breaches_after_limit() {
    // ADR 0150 — one iteration = one executed statement.
    let dir = tempdir().unwrap();
    setup(dir.path());
    let eng = MvccSqlEngine::open(dir.path()).unwrap();
    seed(&eng, 1);
    let err = eng.with_session_budget(QueryBudget {
        max_iterations: Some(2),
        ..QueryBudget::unbounded()
    }, |s| {
        s.execute("BEGIN")?;
        s.execute("SELECT id FROM app.public.items")?;
        s.execute("SELECT id FROM app.public.items")
    }).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Iterations { .. })));
}

#[test]
fn insert_over_byte_budget_errors() {
    // ADR 0149 — DML byte charges.
    let dir = tempdir().unwrap();
    setup(dir.path());
    let eng = MvccSqlEngine::open(dir.path()).unwrap();
    let err = eng.with_session_budget(QueryBudget {
        max_bytes_materialized: Some(5),
        ..QueryBudget::unbounded()
    }, |s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO app.public.items (id, name) VALUES (1, 'hello')")
    }).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Bytes { .. })));
}

#[test]
fn select_over_byte_budget_errors() {
    // ADR 0148 — byte budget breach on the SELECT path.
    let dir = tempdir().unwrap();
    setup(dir.path());
    let eng = MvccSqlEngine::open(dir.path()).unwrap();
    seed(&eng, 3);
    let err = eng.with_session_budget(QueryBudget {
        max_bytes_materialized: Some(0),
        ..QueryBudget::unbounded()
    }, |s| {
        s.execute("BEGIN")?;
        s.execute("SELECT id FROM app.public.items")
    }).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Bytes { .. })),
        "expected Budget::Bytes, got {err:?}");
}

#[test]
fn insert_charges_one_row_per_statement() {
    // ADR 0147 — INSERT now charges 1 row.
    let dir = tempdir().unwrap();
    setup(dir.path());
    let eng = MvccSqlEngine::open(dir.path()).unwrap();
    let err = eng.with_session_budget(QueryBudget {
        max_rows_scanned: Some(1),
        ..QueryBudget::unbounded()
    }, |s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO app.public.items (id, name) VALUES (1, 'a')")?;
        s.execute("INSERT INTO app.public.items (id, name) VALUES (2, 'b')")
    }).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Rows { .. })));
}

#[test]
fn update_and_delete_charge_affected_rows() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let eng = MvccSqlEngine::open(dir.path()).unwrap();
    seed(&eng, 2);
    eng.with_session_budget(QueryBudget {
        max_rows_scanned: Some(4),
        ..QueryBudget::unbounded()
    }, |s| {
        s.execute("BEGIN")?;
        s.execute("SELECT id FROM app.public.items")?;                            // +2
        s.execute("UPDATE app.public.items SET name = 'x' WHERE id = 0")?;        // +1
        s.execute("UPDATE app.public.items SET name = 'x' WHERE id = 9999")?;     // +0
        s.execute("DELETE FROM app.public.items WHERE id = 0")?;                  // +1
        s.execute("COMMIT")
    }).unwrap();
}
