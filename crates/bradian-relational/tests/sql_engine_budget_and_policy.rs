//! ADR 0156 — budget + policy combined constructor
//! exercised across all four SQL engines.

use bradian_core::AuthorizationContext;
use bradian_relational::access_policy::{
    ColumnLevelPolicy, RowLevelPolicy, TableAccessPolicy, TenantIsolation,
};
use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::mvcc_sql::MvccSqlEngine;
use bradian_relational::query_budget::{BudgetExceeded, BudgetTracker, QueryBudget};
use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn setup(root: &std::path::Path) {
    {
        let ddl = SqlEngine::open(root).unwrap();
        for sql in [
            "CREATE DATABASE app",
            "CREATE SCHEMA app.public",
            "CREATE TABLE app.public.items (
                id BIGINT NOT NULL,
                tenant TEXT NOT NULL,
                name TEXT NOT NULL,
                PRIMARY KEY (id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let domain = DomainSqlEngine::open(root).unwrap();
        domain.register_catalog_table("app.public.items").unwrap();
    }
}

fn seed_domain(db: &DomainSqlEngine) {
    db.with_session(|s| {
        s.execute("BEGIN")?;
        for (id, tenant, name) in [(1i64, "acme", "a"), (2, "acme", "b"), (3, "widgets", "c")] {
            s.execute(&format!(
                "INSERT INTO app.public.items (id, tenant, name) VALUES ({id}, '{tenant}', '{name}')"
            ))?;
        }
        s.execute("COMMIT")
    }).unwrap();
}

fn acme_policy() -> TableAccessPolicy {
    TableAccessPolicy {
        row_level: RowLevelPolicy::allow_all(),
        column_level: ColumnLevelPolicy::deny_all(),
        tenant: Some(TenantIsolation::single_tenant("tenant", "acme")),
    }
}

#[test]
fn sql_engine_budget_plus_policy_filters_and_charges() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let eng = SqlEngine::open(dir.path()).unwrap();
    // Seed via the base engine (no session).
    for (id, tenant, name) in [(1i64, "acme", "a"), (2, "acme", "b"), (3, "widgets", "c")] {
        eng.execute(&format!(
            "INSERT INTO app.public.items (id, tenant, name) VALUES ({id}, '{tenant}', '{name}')"
        )).unwrap();
    }
    let mut tracker = BudgetTracker::new(QueryBudget {
        max_rows_scanned: Some(100),
        ..QueryBudget::unbounded()
    });
    let policy = acme_policy();
    let authz = AuthorizationContext::administrator("root");
    let rs = eng.execute_with_budget_and_policy(
        "SELECT id FROM app.public.items", &mut tracker, &policy, &authz).unwrap();
    let SqlResultSet::Rows { rows, .. } = rs else { panic!(); };
    assert_eq!(rows.len(), 2);
    // Budget charged pre-filter (3 rows materialized before the policy ran).
    assert_eq!(tracker.rows_scanned(), 3);
}

#[test]
fn sql_engine_budget_breach_surfaces_despite_policy() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let eng = SqlEngine::open(dir.path()).unwrap();
    for i in 0..5 {
        eng.execute(&format!(
            "INSERT INTO app.public.items (id, tenant, name) VALUES ({i}, 'acme', 'x')"
        )).unwrap();
    }
    let mut tracker = BudgetTracker::new(QueryBudget {
        max_rows_scanned: Some(2),
        ..QueryBudget::unbounded()
    });
    let policy = acme_policy();
    let authz = AuthorizationContext::administrator("root");
    let err = eng.execute_with_budget_and_policy(
        "SELECT id FROM app.public.items", &mut tracker, &policy, &authz).unwrap_err();
    assert!(matches!(err, SqlError::Budget(BudgetExceeded::Rows { .. })));
}

#[test]
fn domain_engine_budget_plus_policy_combined() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let db = DomainSqlEngine::open(dir.path()).unwrap();
    db.register_catalog_table("app.public.items").unwrap();
    seed_domain(&db);
    db.with_session_budget_and_policy(
        QueryBudget { max_rows_scanned: Some(100), ..QueryBudget::unbounded() },
        acme_policy(),
        AuthorizationContext::administrator("root"),
        |s| {
            s.execute("BEGIN")?;
            let rs = s.execute("SELECT id FROM app.public.items")?;
            let SqlResultSet::Rows { rows, .. } = rs else { panic!(); };
            assert_eq!(rows.len(), 2);
            assert!(rows.iter().all(|r| r[0] == Value::Int64(1) || r[0] == Value::Int64(2)));
            s.execute("COMMIT")
        }).unwrap();
}

#[test]
fn mvcc_engine_budget_plus_policy_combined() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let eng = MvccSqlEngine::open(dir.path()).unwrap();
    eng.with_session(|s| {
        s.execute("BEGIN")?;
        for (id, tenant, name) in [(1i64, "acme", "a"), (2, "widgets", "b")] {
            s.execute(&format!(
                "INSERT INTO app.public.items (id, tenant, name) VALUES ({id}, '{tenant}', '{name}')"
            ))?;
        }
        s.execute("COMMIT")
    }).unwrap();
    eng.with_session_budget_and_policy(
        QueryBudget::unbounded(),
        acme_policy(),
        AuthorizationContext::administrator("root"),
        |s| {
            s.execute("BEGIN")?;
            let rs = s.execute("SELECT id FROM app.public.items")?;
            let SqlResultSet::Rows { rows, .. } = rs else { panic!(); };
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], Value::Int64(1));
            s.execute("COMMIT")
        }).unwrap();
}
