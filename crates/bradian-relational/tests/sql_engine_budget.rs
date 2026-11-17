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
fn insert_charges_one_row_and_breaches_over_budget() {
    // ADR 0147 — INSERT now charges 1 row per statement.
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget {
        max_rows_scanned: Some(1),
        ..QueryBudget::unbounded()
    });
    eng.execute_with_budget(
        "INSERT INTO app.public.items (id, name) VALUES (99, 'x')",
        &mut tracker).unwrap();
    assert_eq!(tracker.rows_scanned(), 1);
    let err = eng.execute_with_budget(
        "INSERT INTO app.public.items (id, name) VALUES (100, 'y')",
        &mut tracker).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Rows { .. })));
}

#[test]
fn update_charges_affected_count_on_hit() {
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget::unbounded());
    eng.execute_with_budget(
        "UPDATE app.public.items SET name = 'new' WHERE id = 1",
        &mut tracker).unwrap();
    assert_eq!(tracker.rows_scanned(), 1);
}

#[test]
fn update_charges_zero_on_miss() {
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget::unbounded());
    eng.execute_with_budget(
        "UPDATE app.public.items SET name = 'new' WHERE id = 9999",
        &mut tracker).unwrap();
    assert_eq!(tracker.rows_scanned(), 0);
}

#[test]
fn delete_charges_affected_count_on_hit() {
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget::unbounded());
    eng.execute_with_budget(
        "DELETE FROM app.public.items WHERE id = 1",
        &mut tracker).unwrap();
    assert_eq!(tracker.rows_scanned(), 1);
}

#[test]
fn truncate_charges_every_row_removed() {
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget::unbounded());
    eng.execute_with_budget(
        "TRUNCATE TABLE app.public.items", &mut tracker).unwrap();
    // seed_engine created 5 rows.
    assert_eq!(tracker.rows_scanned(), 5);
}

#[test]
fn select_charges_byte_size_of_materialized_rows() {
    // ADR 0148 — the SELECT path charges bytes alongside
    // rows. seed_engine inserts id=0..4 + "row0".."row4";
    // each row is 8-byte Int64 + ~4-byte string.
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget::unbounded());
    eng.execute_with_budget(
        "SELECT id FROM app.public.items", &mut tracker).unwrap();
    // 5 rows × (8-byte id + 4-byte name) = 60 bytes.
    assert_eq!(tracker.bytes_materialized(), 60);
}

#[test]
fn insert_charges_bytes_of_inserted_row() {
    // ADR 0149 — DML byte charges. INSERT row has
    // Int64(99) (8B) + String("y") (1B) = 9 bytes.
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget::unbounded());
    eng.execute_with_budget(
        "INSERT INTO app.public.items (id, name) VALUES (99, 'y')",
        &mut tracker).unwrap();
    assert_eq!(tracker.bytes_materialized(), 9);
    assert_eq!(tracker.rows_scanned(), 1);
}

#[test]
fn insert_over_byte_budget_errors() {
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget {
        max_bytes_materialized: Some(5),  // too tight for Int64+String
        ..QueryBudget::unbounded()
    });
    let err = eng.execute_with_budget(
        "INSERT INTO app.public.items (id, name) VALUES (99, 'hello')",
        &mut tracker).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Bytes { .. })));
}

#[test]
fn update_charges_bytes_of_written_row() {
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget::unbounded());
    // UPDATE id=1 SET name='AB' → row = Int64 (8) + "AB" (2) = 10.
    eng.execute_with_budget(
        "UPDATE app.public.items SET name = 'AB' WHERE id = 1",
        &mut tracker).unwrap();
    assert_eq!(tracker.bytes_materialized(), 10);
}

#[test]
fn delete_charges_bytes_of_removed_row() {
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget::unbounded());
    // seed inserts id=1 name='row1' → 8 + 4 = 12 bytes.
    eng.execute_with_budget(
        "DELETE FROM app.public.items WHERE id = 1",
        &mut tracker).unwrap();
    assert_eq!(tracker.bytes_materialized(), 12);
}

#[test]
fn truncate_charges_sum_of_all_rows() {
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget::unbounded());
    // 5 rows × (8 + 4) = 60 bytes.
    eng.execute_with_budget(
        "TRUNCATE TABLE app.public.items", &mut tracker).unwrap();
    assert_eq!(tracker.bytes_materialized(), 60);
    assert_eq!(tracker.rows_scanned(), 5);
}

#[test]
fn insert_over_byte_budget_is_rejected_before_storage_side_effect() {
    // ADR 0151 — preflight: a budget breach must prevent the
    // mutation from reaching storage. After the rejected
    // INSERT, the row must NOT be visible.
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget {
        max_bytes_materialized: Some(5),  // too tight
        ..QueryBudget::unbounded()
    });
    let err = eng.execute_with_budget(
        "INSERT INTO app.public.items (id, name) VALUES (1000, 'rejected')",
        &mut tracker).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Bytes { .. })));
    // Verify the row is NOT present (no post-commit leak).
    let rs = eng.execute(
        "SELECT id FROM app.public.items WHERE id = 1000").unwrap();
    let SqlResultSet::Rows { rows, .. } = rs else { panic!("expected rows"); };
    assert!(rows.is_empty(), "rejected INSERT must not leave a row behind");
}

#[test]
fn delete_over_byte_budget_is_rejected_before_storage_side_effect() {
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget {
        max_bytes_materialized: Some(5),
        ..QueryBudget::unbounded()
    });
    let err = eng.execute_with_budget(
        "DELETE FROM app.public.items WHERE id = 1",
        &mut tracker).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Bytes { .. })));
    // Verify id=1 still exists.
    let rs = eng.execute(
        "SELECT id FROM app.public.items WHERE id = 1").unwrap();
    let SqlResultSet::Rows { rows, .. } = rs else { panic!(); };
    assert_eq!(rows.len(), 1, "rejected DELETE must not remove the row");
}

#[test]
fn iteration_counter_increments_per_statement() {
    // ADR 0150 — one iteration = one executed statement.
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget::unbounded());
    eng.execute_with_budget("SELECT id FROM app.public.items", &mut tracker).unwrap();
    eng.execute_with_budget("SELECT id FROM app.public.items", &mut tracker).unwrap();
    eng.execute_with_budget("SELECT id FROM app.public.items", &mut tracker).unwrap();
    assert_eq!(tracker.iterations(), 3);
}

#[test]
fn iteration_cap_breaches_after_limit() {
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget {
        max_iterations: Some(2),
        ..QueryBudget::unbounded()
    });
    eng.execute_with_budget("SELECT id FROM app.public.items", &mut tracker).unwrap();
    eng.execute_with_budget("SELECT id FROM app.public.items", &mut tracker).unwrap();
    let err = eng.execute_with_budget(
        "SELECT id FROM app.public.items", &mut tracker).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Iterations { .. })));
}

#[test]
fn select_over_byte_budget_errors_with_sqlerror_budget() {
    let (_d, eng) = seed_engine();
    let mut tracker = BudgetTracker::new(QueryBudget {
        max_bytes_materialized: Some(10),  // too tight
        ..QueryBudget::unbounded()
    });
    let err = eng.execute_with_budget(
        "SELECT id FROM app.public.items", &mut tracker).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Bytes { .. })),
        "expected Budget::Bytes, got {err:?}");
}
