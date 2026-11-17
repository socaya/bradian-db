//! ADR 0143 — engine-level integration of BudgetTracker
//! into the historical SELECT path.

#[path = "support/epoch_fixture.rs"]
#[allow(dead_code)]
mod fixture;
use fixture::migrated;

use std::time::Duration;

use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::query_budget::{BudgetExceeded, QueryBudget};
use bradian_relational::sql::{SqlError, SqlResultSet};

#[test]
fn select_within_budget_succeeds() {
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    db.with_session_budget(QueryBudget {
        max_rows_scanned: Some(1_000),
        max_duration: Some(Duration::from_secs(30)),
        ..QueryBudget::unbounded()
    }, |s| {
        s.execute("BEGIN")?;
        let rs = s.execute("SELECT id FROM bank.public.audit")?;
        assert!(matches!(rs, SqlResultSet::Rows { rows, .. } if !rows.is_empty()));
        s.execute("COMMIT")
    }).unwrap();
}

#[test]
fn select_over_row_budget_errors_with_sqlerror_budget() {
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    let err = db.with_session_budget(QueryBudget {
        max_rows_scanned: Some(0),  // 0-row budget → first scan breaches
        ..QueryBudget::unbounded()
    }, |s| {
        s.execute("BEGIN")?;
        s.execute("SELECT id FROM bank.public.audit")
    }).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Rows { .. })),
        "expected BudgetExceeded::Rows, got {err:?}");
}

#[test]
fn select_over_deadline_errors_with_sqlerror_budget() {
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    let err = db.with_session_budget(QueryBudget {
        max_duration: Some(Duration::from_millis(1)),
        ..QueryBudget::unbounded()
    }, |s| {
        s.execute("BEGIN")?;
        // Sleep past the deadline before issuing the SELECT
        // so the pre-scan deadline check fires.
        std::thread::sleep(Duration::from_millis(50));
        s.execute("SELECT id FROM bank.public.audit")
    }).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Duration { .. })),
        "expected BudgetExceeded::Duration, got {err:?}");
}

#[test]
fn unbounded_budget_never_errors() {
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    db.with_session_budget(QueryBudget::unbounded(), |s| {
        s.execute("BEGIN")?;
        s.execute("SELECT id FROM bank.public.audit")?;
        s.execute("SELECT id FROM bank.public.audit")?;
        s.execute("SELECT id FROM bank.public.audit")?;
        s.execute("COMMIT")
    }).unwrap();
}

#[test]
fn budget_accumulates_across_statements_in_one_session() {
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    // The audit fixture has 1 row; cap at 1 — first SELECT
    // (1 row) is OK, second SELECT pushes cumulative to 2
    // which breaches.
    let err = db.with_session_budget(QueryBudget {
        max_rows_scanned: Some(1),
        ..QueryBudget::unbounded()
    }, |s| {
        s.execute("BEGIN")?;
        s.execute("SELECT id FROM bank.public.audit")?;  // 1 row
        s.execute("SELECT id FROM bank.public.audit")    // +1 → breach
    }).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Rows { .. })),
        "expected BudgetExceeded::Rows, got {err:?}");
}

#[test]
fn existing_sessions_without_budget_are_unchanged() {
    // Sanity: the additive integration must not break the
    // no-budget path.
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("SELECT id FROM bank.public.audit")?;
        s.execute("COMMIT")
    }).unwrap();
}

#[test]
fn update_charges_affected_count() {
    // ADR 0147 — non-SELECT paths now charge the budget.
    // audit has 1 row with id=9. UPDATE WHERE id=9 touches
    // 1 row; UPDATE WHERE id=9999 touches 0.
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    let err = db.with_session_budget(QueryBudget {
        max_rows_scanned: Some(1),  // 1 hit allowed
        ..QueryBudget::unbounded()
    }, |s| {
        s.execute("BEGIN")?;
        s.execute("UPDATE bank.public.audit SET balance = 1 WHERE id = 9999")?; // +0
        s.execute("UPDATE bank.public.audit SET balance = 1 WHERE id = 9")?;    // +1
        s.execute("UPDATE bank.public.audit SET balance = 2 WHERE id = 9")      // +1 → 2 > 1
    }).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Rows { .. })),
        "expected Budget::Rows, got {err:?}");
}

#[test]
fn select_over_byte_budget_errors() {
    // ADR 0148 — the SELECT path charges bytes alongside rows.
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    let err = db.with_session_budget(QueryBudget {
        max_bytes_materialized: Some(0),  // 0 bytes allowed → any row breaches
        ..QueryBudget::unbounded()
    }, |s| {
        s.execute("BEGIN")?;
        s.execute("SELECT id FROM bank.public.audit")
    }).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Bytes { .. })),
        "expected Budget::Bytes, got {err:?}");
}

#[test]
fn delete_charges_affected_count() {
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    db.with_session_budget(QueryBudget {
        max_rows_scanned: Some(1),
        ..QueryBudget::unbounded()
    }, |s| {
        s.execute("BEGIN")?;
        s.execute("DELETE FROM bank.public.audit WHERE id = 9999")?;  // +0
        s.execute("DELETE FROM bank.public.audit WHERE id = 9")?;     // +1
        s.execute("ROLLBACK")
    }).unwrap();
}
