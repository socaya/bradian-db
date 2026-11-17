//! ADR 0144 — engine-level integration of BudgetTracker
//! into the base SqlEngine SELECT path.

use std::time::Duration;

use bradian_relational::query_budget::{BudgetExceeded, BudgetTracker, QueryBudget};
use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};

fn seed_engine() -> (tempfile::TempDir, SqlEngine) {
    let dir = tempfile::tempdir().unwrap();
    let eng = SqlEngine::open(dir.path()).unwrap();
    eng.execute("CREATE DATABASE app").unwrap();
    eng.execute("CREATE SCHEMA app.public").unwrap();
    eng.execute("CREATE TABLE app.public.items (
                     id BIGINT NOT NULL,
                     name TEXT NOT NULL,
                     PRIMARY KEY (id))").unwrap();
    for i in 0..5 {
        eng.execute(&format!(
            "INSERT INTO app.public.items (id, name) VALUES ({i}, 'row{i}')"
        )).unwrap();
    }
    (dir, eng)
}

#[test]
fn select_within_budget_succeeds() {
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget {
        max_rows_scanned: Some(100),
        max_duration: Some(Duration::from_secs(30)),
        ..QueryBudget::unbounded()
    });
    let rs = eng.execute_with_budget(
        "SELECT id FROM app.public.items", &mut tracker).unwrap();
    assert!(matches!(rs, SqlResultSet::Rows { rows, .. } if rows.len() == 5));
    assert_eq!(tracker.rows_scanned(), 5);
}

#[test]
fn select_over_row_budget_errors_with_sqlerror_budget() {
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget {
        max_rows_scanned: Some(2),
        ..QueryBudget::unbounded()
    });
    let err = eng.execute_with_budget(
        "SELECT id FROM app.public.items", &mut tracker).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Rows { .. })),
        "expected Budget::Rows, got {err:?}");
}

#[test]
fn select_over_deadline_errors_with_sqlerror_budget() {
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget {
        max_duration: Some(Duration::from_millis(1)),
        ..QueryBudget::unbounded()
    });
    std::thread::sleep(Duration::from_millis(50));
    let err = eng.execute_with_budget(
        "SELECT id FROM app.public.items", &mut tracker).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Duration { .. })),
        "expected Budget::Duration, got {err:?}");
}

#[test]
fn unbounded_budget_never_errors() {
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget::unbounded());
    for _ in 0..3 {
        eng.execute_with_budget(
            "SELECT id FROM app.public.items", &mut tracker).unwrap();
    }
    assert_eq!(tracker.rows_scanned(), 15);
}

#[test]
fn budget_accumulates_across_successive_selects() {
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget {
        max_rows_scanned: Some(7),  // 5 + 5 = 10 breaches
        ..QueryBudget::unbounded()
    });
    eng.execute_with_budget(
        "SELECT id FROM app.public.items", &mut tracker).unwrap();   // +5 -> 5
    let err = eng.execute_with_budget(
        "SELECT id FROM app.public.items", &mut tracker).unwrap_err(); // +5 -> 10
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Rows { .. })));
}

#[test]
fn execute_without_budget_path_unchanged() {
    let (_d, eng) = seed_engine();
    let rs = eng.execute("SELECT id FROM app.public.items").unwrap();
    assert!(matches!(rs, SqlResultSet::Rows { rows, .. } if rows.len() == 5));
}

#[test]
fn budget_does_not_charge_non_select_statements() {
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget {
        max_rows_scanned: Some(0),  // no rows allowed
        ..QueryBudget::unbounded()
    });
    // INSERT path doesn't consult the budget — this still
    // succeeds despite the 0-row cap.
    let rs = eng.execute_with_budget(
        "INSERT INTO app.public.items (id, name) VALUES (99, 'x')",
        &mut tracker).unwrap();
    assert!(matches!(rs, SqlResultSet::Command { affected: 1 }));
    assert_eq!(tracker.rows_scanned(), 0);
}
