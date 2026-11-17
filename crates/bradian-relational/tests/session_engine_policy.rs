//! ADR 0154 — row/column/tenant policy wiring into the
//! three session-based SQL engines (HistorySqlEngine,
//! DomainSqlEngine, MvccSqlEngine).

use bradian_core::AuthorizationContext;
use bradian_relational::access_policy::{
    ColumnLevelPolicy, RowLevelPolicy, RowPredicate, TableAccessPolicy, TenantIsolation,
};
use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::mvcc_sql::MvccSqlEngine;
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
        for (id, tenant, name) in [
            (1i64, "acme", "alpha"),
            (2, "acme", "beta"),
            (3, "widgets", "gamma"),
        ] {
            s.execute(&format!(
                "INSERT INTO app.public.items (id, tenant, name) VALUES ({id}, '{tenant}', '{name}')"
            ))?;
        }
        s.execute("COMMIT")
    }).unwrap();
}

fn seed_mvcc(eng: &MvccSqlEngine) {
    eng.with_session(|s| {
        s.execute("BEGIN")?;
        for (id, tenant, name) in [
            (1i64, "acme", "alpha"),
            (2, "acme", "beta"),
            (3, "widgets", "gamma"),
        ] {
            s.execute(&format!(
                "INSERT INTO app.public.items (id, tenant, name) VALUES ({id}, '{tenant}', '{name}')"
            ))?;
        }
        s.execute("COMMIT")
    }).unwrap();
}

fn tenant_policy(t: &str) -> TableAccessPolicy {
    TableAccessPolicy {
        row_level: RowLevelPolicy::allow_all(),
        column_level: ColumnLevelPolicy::deny_all(),
        tenant: Some(TenantIsolation::single_tenant("tenant", t)),
    }
}

// ---------------- DomainSqlEngine ----------------

#[test]
fn domain_tenant_isolation_filters_other_tenants() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let db = DomainSqlEngine::open(dir.path()).unwrap();
    db.register_catalog_table("app.public.items").unwrap();
    seed_domain(&db);
    let policy = tenant_policy("acme");
    let authz = AuthorizationContext::administrator("root");
    db.with_session_policy(policy, authz, |s| {
        s.execute("BEGIN")?;
        let rs = s.execute("SELECT id FROM app.public.items")?;
        let SqlResultSet::Rows { rows, .. } = rs else { panic!(); };
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r[0] == Value::Int64(1) || r[0] == Value::Int64(2)));
        s.execute("COMMIT")
    }).unwrap();
}

#[test]
fn domain_always_deny_filters_everything() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let db = DomainSqlEngine::open(dir.path()).unwrap();
    db.register_catalog_table("app.public.items").unwrap();
    seed_domain(&db);
    let policy = TableAccessPolicy {
        row_level: RowLevelPolicy { predicate: RowPredicate::AlwaysDeny },
        column_level: ColumnLevelPolicy::deny_all(),
        tenant: None,
    };
    let authz = AuthorizationContext::administrator("root");
    db.with_session_policy(policy, authz, |s| {
        s.execute("BEGIN")?;
        let rs = s.execute("SELECT id FROM app.public.items")?;
        let SqlResultSet::Rows { rows, .. } = rs else { panic!(); };
        assert!(rows.is_empty());
        s.execute("COMMIT")
    }).unwrap();
}

#[test]
fn domain_policy_error_surfaces_via_sqlerror() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let db = DomainSqlEngine::open(dir.path()).unwrap();
    db.register_catalog_table("app.public.items").unwrap();
    seed_domain(&db);
    let policy = TableAccessPolicy {
        row_level: RowLevelPolicy {
            predicate: RowPredicate::ColumnEquals {
                column: "missing".into(),
                value: Value::Int64(0),
            },
        },
        column_level: ColumnLevelPolicy::deny_all(),
        tenant: None,
    };
    let authz = AuthorizationContext::administrator("root");
    let err = db.with_session_policy(policy, authz, |s| {
        s.execute("BEGIN")?;
        s.execute("SELECT id FROM app.public.items")
    }).unwrap_err();
    assert!(matches!(err, SqlError::Policy(_)));
}

// ---------------- MvccSqlEngine ----------------

#[test]
fn mvcc_tenant_isolation_filters_other_tenants() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let eng = MvccSqlEngine::open(dir.path()).unwrap();
    seed_mvcc(&eng);
    let policy = tenant_policy("widgets");
    let authz = AuthorizationContext::administrator("root");
    eng.with_session_policy(policy, authz, |s| {
        s.execute("BEGIN")?;
        let rs = s.execute("SELECT id FROM app.public.items")?;
        let SqlResultSet::Rows { rows, .. } = rs else { panic!(); };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], Value::Int64(3));
        s.execute("COMMIT")
    }).unwrap();
}

#[test]
fn mvcc_without_policy_returns_every_row() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let eng = MvccSqlEngine::open(dir.path()).unwrap();
    seed_mvcc(&eng);
    eng.with_session(|s| {
        s.execute("BEGIN")?;
        let rs = s.execute("SELECT id FROM app.public.items")?;
        let SqlResultSet::Rows { rows, .. } = rs else { panic!(); };
        assert_eq!(rows.len(), 3);
        s.execute("COMMIT")
    }).unwrap();
}

// ---------------- HistorySqlEngine ----------------

#[path = "support/epoch_fixture.rs"]
#[allow(dead_code)]
mod history_fixture;

#[test]
fn history_policy_filters_select_rows() {
    let (_dir, root) = history_fixture::migrated();
    let db = bradian_relational::history_sql::HistorySqlEngine::open(&root).unwrap();
    // audit has 1 row id=9; AlwaysDeny strips it.
    let policy = TableAccessPolicy {
        row_level: RowLevelPolicy { predicate: RowPredicate::AlwaysDeny },
        column_level: ColumnLevelPolicy::deny_all(),
        tenant: None,
    };
    let authz = AuthorizationContext::administrator("root");
    db.with_session_policy(policy, authz, |s| {
        s.execute("BEGIN")?;
        let rs = s.execute("SELECT id FROM bank.public.audit")?;
        let SqlResultSet::Rows { rows, .. } = rs else { panic!(); };
        assert!(rows.is_empty(), "AlwaysDeny must strip all rows");
        s.execute("COMMIT")
    }).unwrap();
}

#[test]
fn history_without_policy_returns_rows() {
    let (_dir, root) = history_fixture::migrated();
    let db = bradian_relational::history_sql::HistorySqlEngine::open(&root).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        let rs = s.execute("SELECT id FROM bank.public.audit")?;
        let SqlResultSet::Rows { rows, .. } = rs else { panic!(); };
        assert!(!rows.is_empty());
        s.execute("COMMIT")
    }).unwrap();
}
