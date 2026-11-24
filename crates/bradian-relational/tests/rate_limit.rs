//! ADR 0285 — per-principal rate limiting on SqlEngine::execute_as.

use bradian_core::AuthorizationContext;
use bradian_relational::sql::{SqlEngine, SqlError};
use tempfile::tempdir;

fn seed() -> (tempfile::TempDir, SqlEngine) {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
    db.execute("CREATE DATABASE t").unwrap();
    db.execute("CREATE SCHEMA t.public").unwrap();
    db.execute("CREATE TABLE t.public.items (id BIGINT NOT NULL, PRIMARY KEY(id))").unwrap();
    (dir, db)
}

#[test]
fn no_policy_permits_unlimited_requests() {
    let (_dir, db) = seed();
    let authz = AuthorizationContext::administrator("alice");
    for _ in 0..1000 {
        db.execute_as(&authz, "SELECT id FROM t.public.items").unwrap();
    }
}

#[test]
fn policy_with_capacity_1_throttles_second_request() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap().with_rate_limit(1, 0);
    db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
    db.execute("CREATE DATABASE t").unwrap();
    db.execute("CREATE SCHEMA t.public").unwrap();
    db.execute("CREATE TABLE t.public.items (id BIGINT NOT NULL, PRIMARY KEY(id))").unwrap();
    // Those 4 setup calls above went through execute() not
    // execute_as(), so they don't charge the bucket.
    let authz = AuthorizationContext::administrator("alice");
    db.execute_as(&authz, "SELECT id FROM t.public.items").unwrap();
    let err = db.execute_as(&authz, "SELECT id FROM t.public.items").unwrap_err();
    assert!(matches!(err, SqlError::Resource(_)), "got {err:?}");
}

#[test]
fn per_principal_isolation() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap().with_rate_limit(1, 0);
    db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
    db.execute("CREATE USER bob WITH PASSWORD 'pw'").unwrap();
    let a = AuthorizationContext::administrator("alice");
    let b = AuthorizationContext::administrator("bob");
    db.execute_as(&a, "SELECT 1 FROM alice").unwrap_err();  // name resolution fails but still charges
    // bob still has his full token.
    let err = db.execute_as(&b, "SELECT 1 FROM alice").unwrap_err();
    // bob's statement still fails resolution, but NOT with Resource.
    assert!(!matches!(err, SqlError::Resource(_)), "got {err:?}");
}

#[test]
fn authz_denied_still_charges_bucket() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap().with_rate_limit(2, 0);
    db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
    // alice has no caps (not the bootstrap user) if a second user was there first.
    // Build a read-only context (allows SELECT only).
    let authz = AuthorizationContext::read_only("eve");
    // DDL under read-only is denied: Authz error BUT the bucket is charged.
    let _ = db.execute_as(&authz, "CREATE DATABASE other");
    let _ = db.execute_as(&authz, "CREATE DATABASE another");
    // Third call should fail rate-limited (bucket drained).
    let err = db.execute_as(&authz, "CREATE DATABASE third").unwrap_err();
    assert!(matches!(err, SqlError::Resource(_)), "got {err:?}");
}
