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
fn cls_allow_list_filters_select_star_output_columns() {
    // ADR 0155 — SELECT * with a CLS allow-list keeps
    // only the readable columns in the output.
    let (_d, eng) = seed_engine();
    let readable: std::collections::BTreeSet<String> =
        ["id".into(), "name".into()].into_iter().collect();
    let policy = TableAccessPolicy {
        row_level: RowLevelPolicy::allow_all(),
        column_level: ColumnLevelPolicy { readable },
        tenant: None,
    };
    let authz = AuthorizationContext::administrator("root");
    let rs = eng.execute_with_policy(
        "SELECT * FROM app.public.items", &policy, &authz).unwrap();
    let SqlResultSet::Rows { columns, .. } = rs else { panic!(); };
    // tenant is omitted.
    assert_eq!(columns, vec!["id".to_string(), "name".to_string()]);
}

#[test]
fn cls_allow_list_rejects_explicit_denied_column() {
    // ADR 0155 — SELECT tenant where "tenant" is not in
    // the allow-list must error with ColumnDenied.
    let (_d, eng) = seed_engine();
    let readable: std::collections::BTreeSet<String> =
        ["id".into(), "name".into()].into_iter().collect();
    let policy = TableAccessPolicy {
        row_level: RowLevelPolicy::allow_all(),
        column_level: ColumnLevelPolicy { readable },
        tenant: None,
    };
    let authz = AuthorizationContext::administrator("root");
    let err = eng.execute_with_policy(
        "SELECT tenant FROM app.public.items", &policy, &authz).unwrap_err();
    match err {
        bradian_relational::sql::SqlError::Policy(
            bradian_relational::access_policy::PolicyError::ColumnDenied { column }
        ) => assert_eq!(column, "tenant"),
        other => panic!("expected ColumnDenied, got {other:?}"),
    }
}

#[test]
fn cls_empty_readable_set_is_treated_as_cls_disabled() {
    // ADR 0155 — deny_all() = empty readable set =
    // "CLS not configured", so no filtering happens.
    // This preserves the ADR 0153/0154 placeholder pattern
    // where deny_all() was used as a dimension-isolation
    // stand-in.
    let (_d, eng) = seed_engine();
    let policy = TableAccessPolicy {
        row_level: RowLevelPolicy::allow_all(),
        column_level: ColumnLevelPolicy::deny_all(),  // empty
        tenant: None,
    };
    let authz = AuthorizationContext::administrator("root");
    let rs = eng.execute_with_policy(
        "SELECT * FROM app.public.items", &policy, &authz).unwrap();
    let SqlResultSet::Rows { columns, rows } = rs else { panic!(); };
    // Full schema returned.
    assert_eq!(columns, vec!["id".to_string(), "tenant".to_string(), "name".to_string()]);
    assert_eq!(rows.len(), 3);
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
