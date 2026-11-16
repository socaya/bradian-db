//! ADR 0098 — SqlEngine::execute_as authz threading.

use bradian_core::{AuthorizationContext, Capability};
use bradian_relational::sql::{SqlEngine, SqlError};
use tempfile::tempdir;

fn open(dir: &std::path::Path) -> SqlEngine {
    SqlEngine::open(dir).unwrap()
}

#[test]
fn administrator_can_create_and_select() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    let a = AuthorizationContext::administrator("root");
    db.execute_as(&a, "CREATE DATABASE test").unwrap();
    db.execute_as(&a, "CREATE SCHEMA test.public").unwrap();
    db.execute_as(&a, "CREATE TABLE test.public.t(id BIGINT NOT NULL, PRIMARY KEY(id))").unwrap();
    db.execute_as(&a, "INSERT INTO test.public.t(id) VALUES(1)").unwrap();
    let r = db.execute_as(&a, "SELECT id FROM test.public.t").unwrap();
    use bradian_relational::sql::SqlResultSet;
    match r {
        SqlResultSet::Rows { rows, .. } => assert_eq!(rows.len(), 1),
        other => panic!("expected rows, got {other:?}"),
    }
}

#[test]
fn read_only_rejects_create_with_authz_denied() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    let r = AuthorizationContext::read_only("analyst");
    let err = db.execute_as(&r, "CREATE DATABASE test").unwrap_err();
    match err {
        SqlError::Authz(denied) => {
            assert_eq!(denied.principal_id, "analyst");
            assert_eq!(denied.required, Capability::Ddl);
        }
        other => panic!("expected Authz denial; got {other:?}"),
    }
}

#[test]
fn read_only_rejects_insert_with_authz_denied() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    // Set up schema as admin first.
    let a = AuthorizationContext::administrator("root");
    db.execute_as(&a, "CREATE DATABASE test").unwrap();
    db.execute_as(&a, "CREATE SCHEMA test.public").unwrap();
    db.execute_as(&a, "CREATE TABLE test.public.t(id BIGINT NOT NULL, PRIMARY KEY(id))").unwrap();

    let r = AuthorizationContext::read_only("analyst");
    let err = db.execute_as(&r, "INSERT INTO test.public.t(id) VALUES(1)").unwrap_err();
    match err {
        SqlError::Authz(denied) => assert_eq!(denied.required, Capability::Write),
        other => panic!("expected Authz denial; got {other:?}"),
    }
    // But select is allowed.
    let _ = db.execute_as(&r, "SELECT id FROM test.public.t").unwrap();
}

#[test]
fn custom_capabilities_allow_write_but_reject_ddl() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    let a = AuthorizationContext::administrator("root");
    db.execute_as(&a, "CREATE DATABASE test").unwrap();
    db.execute_as(&a, "CREATE SCHEMA test.public").unwrap();
    db.execute_as(&a, "CREATE TABLE test.public.t(id BIGINT NOT NULL, PRIMARY KEY(id))").unwrap();

    // "appuser" can read and write, but cannot DDL.
    let w = AuthorizationContext::new("appuser",
        vec![Capability::Read, Capability::Write]);
    db.execute_as(&w, "INSERT INTO test.public.t(id) VALUES(1)").unwrap();
    let err = db.execute_as(&w, "CREATE DATABASE other").unwrap_err();
    match err {
        SqlError::Authz(d) => assert_eq!(d.required, Capability::Ddl),
        other => panic!("expected Authz; got {other:?}"),
    }
}

#[test]
fn unknown_sql_keyword_passes_authz_and_lets_parser_produce_the_error() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    let r = AuthorizationContext::read_only("analyst");
    // "FOOBAR …" is not a recognised keyword. Authz does not
    // block it; the parser should produce a SqlError::Syntax.
    let err = db.execute_as(&r, "FOOBAR 1").unwrap_err();
    match err {
        SqlError::Syntax(_) => {}
        SqlError::Unsupported(_) => {}
        SqlError::Resolution(_) => {}
        other => panic!("expected Syntax/Unsupported/Resolution; got {other:?}"),
    }
}
