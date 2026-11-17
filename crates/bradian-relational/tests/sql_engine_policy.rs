//! ADR 0153 — engine-level integration of ADR 0123
//! `TableAccessPolicy` into the base SqlEngine SELECT
//! path.

use bradian_core::AuthorizationContext;
use bradian_relational::access_policy::{
    ColumnLevelPolicy, RowLevelPolicy, RowPredicate, TableAccessPolicy, TenantIsolation,
};
use bradian_relational::sql::{SqlEngine, SqlResultSet};
use bradian_relational::typed_table::Value;

fn seed_engine() -> (tempfile::TempDir, SqlEngine) {
    let dir = tempfile::tempdir().unwrap();
    let eng = SqlEngine::open(dir.path()).unwrap();
    eng.execute("CREATE DATABASE app").unwrap();
    eng.execute("CREATE SCHEMA app.public").unwrap();
    eng.execute("CREATE TABLE app.public.items (
                     id BIGINT NOT NULL,
                     tenant TEXT NOT NULL,
                     name TEXT NOT NULL,
                     PRIMARY KEY (id))").unwrap();
    for (id, tenant, name) in [
        (1, "acme", "alpha"),
        (2, "acme", "beta"),
        (3, "widgets", "gamma"),
    ] {
        eng.execute(&format!(
            "INSERT INTO app.public.items (id, tenant, name) VALUES ({id}, '{tenant}', '{name}')"
        )).unwrap();
    }
    (dir, eng)
}

#[test]
fn allow_all_policy_returns_every_row() {
    let (_d, eng) = seed_engine();
    let policy = TableAccessPolicy {
        row_level: RowLevelPolicy::allow_all(),
        column_level: ColumnLevelPolicy::deny_all(),  // unused: projection is caller-side
        tenant: None,
    };
    let authz = AuthorizationContext::administrator("root");
    let rs = eng.execute_with_policy(
        "SELECT id FROM app.public.items", &policy, &authz).unwrap();
    let SqlResultSet::Rows { rows, .. } = rs else { panic!(); };
    assert_eq!(rows.len(), 3);
}

#[test]
fn always_deny_row_policy_filters_everything() {
    let (_d, eng) = seed_engine();
    let policy = TableAccessPolicy {
        row_level: RowLevelPolicy { predicate: RowPredicate::AlwaysDeny },
        column_level: ColumnLevelPolicy::deny_all(),
        tenant: None,
    };
    let authz = AuthorizationContext::administrator("root");
    let rs = eng.execute_with_policy(
        "SELECT id FROM app.public.items", &policy, &authz).unwrap();
    let SqlResultSet::Rows { rows, .. } = rs else { panic!(); };
    assert!(rows.is_empty());
}

#[test]
fn tenant_isolation_filters_other_tenants() {
    let (_d, eng) = seed_engine();
    let policy = TableAccessPolicy {
        row_level: RowLevelPolicy::allow_all(),
        column_level: ColumnLevelPolicy::deny_all(),
        tenant: Some(TenantIsolation::single_tenant("tenant", "acme")),
    };
    let authz = AuthorizationContext::administrator("root");
    let rs = eng.execute_with_policy(
        "SELECT id FROM app.public.items", &policy, &authz).unwrap();
    let SqlResultSet::Rows { rows, .. } = rs else { panic!(); };
    // Only id=1 and id=2 (tenant=acme).
    assert_eq!(rows.len(), 2);
    let ids: Vec<_> = rows.iter().map(|r| r[0].clone()).collect();
    assert!(ids.contains(&Value::Int64(1)));
    assert!(ids.contains(&Value::Int64(2)));
}

#[test]
fn row_level_column_equals_filter() {
    let (_d, eng) = seed_engine();
    let policy = TableAccessPolicy {
        row_level: RowLevelPolicy {
            predicate: RowPredicate::ColumnEquals {
                column: "name".into(),
                value: Value::String("beta".into()),
            },
        },
        column_level: ColumnLevelPolicy::deny_all(),
        tenant: None,
    };
    let authz = AuthorizationContext::administrator("root");
    let rs = eng.execute_with_policy(
        "SELECT id FROM app.public.items", &policy, &authz).unwrap();
    let SqlResultSet::Rows { rows, .. } = rs else { panic!(); };
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Int64(2));  // name='beta' is id=2
}

#[test]
fn row_level_unknown_column_errors_through_sqlerror() {
    let (_d, eng) = seed_engine();
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
    let err = eng.execute_with_policy(
        "SELECT id FROM app.public.items", &policy, &authz).unwrap_err();
    // Comes through as SqlError::Policy.
    match err {
        bradian_relational::sql::SqlError::Policy(_) => {}
        other => panic!("expected SqlError::Policy, got {other:?}"),
    }
}

#[test]
fn execute_without_policy_returns_every_row() {
    // Sanity: the additive integration must not break the
    // no-policy path.
    let (_d, eng) = seed_engine();
    let rs = eng.execute("SELECT id FROM app.public.items").unwrap();
    let SqlResultSet::Rows { rows, .. } = rs else { panic!(); };
    assert_eq!(rows.len(), 3);
}

#[test]
fn where_clause_and_policy_both_apply() {
    // WHERE id=1 hits one row; policy's tenant=widgets
    // filters it out. Result: empty.
    let (_d, eng) = seed_engine();
    let policy = TableAccessPolicy {
        row_level: RowLevelPolicy::allow_all(),
        column_level: ColumnLevelPolicy::deny_all(),
        tenant: Some(TenantIsolation::single_tenant("tenant", "widgets")),
    };
    let authz = AuthorizationContext::administrator("root");
    let rs = eng.execute_with_policy(
        "SELECT id FROM app.public.items WHERE id = 1", &policy, &authz).unwrap();
    let SqlResultSet::Rows { rows, .. } = rs else { panic!(); };
    assert!(rows.is_empty());
}
