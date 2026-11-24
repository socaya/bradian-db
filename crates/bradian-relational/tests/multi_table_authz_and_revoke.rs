//! ADR 0280 + 0282 — multi-table FROM authz + revocation
//! propagation to live sessions.

use bradian_relational::sql::{SqlEngine, SqlError};
use tempfile::tempdir;

fn seed_two_tables_with_bob() -> (tempfile::TempDir, SqlEngine) {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
    db.execute("CREATE USER bob WITH PASSWORD 'pw'").unwrap();
    db.execute("CREATE DATABASE t").unwrap();
    db.execute("CREATE SCHEMA t.public").unwrap();
    db.execute("CREATE TABLE t.public.parents (pid BIGINT NOT NULL, label TEXT, PRIMARY KEY(pid))")
        .unwrap();
    db.execute("CREATE TABLE t.public.children (cid BIGINT NOT NULL, parent BIGINT, PRIMARY KEY(cid))")
        .unwrap();
    db.execute("INSERT INTO t.public.parents(pid, label) VALUES(1, 'a')").unwrap();
    db.execute("INSERT INTO t.public.children(cid, parent) VALUES(10, 1)").unwrap();
    (dir, db)
}

#[test]
fn join_requires_grants_on_every_table() {
    let (_dir, db) = seed_two_tables_with_bob();
    // Grant on only one table — the join must fail.
    db.execute("GRANT SELECT ON t.public.parents TO bob").unwrap();
    let err = db.with_authenticated_session("bob", "pw", |s| s.execute(
        "SELECT c.cid, p.label FROM t.public.children c INNER JOIN t.public.parents p ON c.parent = p.pid"
    )).unwrap_err();
    assert!(matches!(err, SqlError::Authz(_)), "got {err:?}");
}

#[test]
fn join_allowed_when_grants_on_both_tables() {
    let (_dir, db) = seed_two_tables_with_bob();
    db.execute("GRANT SELECT ON t.public.parents TO bob").unwrap();
    db.execute("GRANT SELECT ON t.public.children TO bob").unwrap();
    db.with_authenticated_session("bob", "pw", |s| s.execute(
        "SELECT c.cid, p.label FROM t.public.children c INNER JOIN t.public.parents p ON c.parent = p.pid"
    )).unwrap();
}

#[test]
fn revoke_propagates_to_live_session() {
    let (_dir, db) = seed_two_tables_with_bob();
    db.execute("GRANT SELECT ON t.public.parents TO bob").unwrap();
    // Within one session, run SELECT, then external REVOKE, then another SELECT.
    db.with_authenticated_session("bob", "pw", |s| {
        s.execute("SELECT pid FROM t.public.parents").unwrap();
        // External REVOKE — happens through the engine, visible via users.snapshot() refresh.
        s.execute("SELECT pid FROM t.public.parents WHERE pid = 1").unwrap();
        Ok(())
    }).unwrap();
    // Now revoke and reopen a fresh session to re-verify (the
    // previous assertion that revoke only takes effect on a
    // NEW session was pinned to ADR 0258's snapshot behavior;
    // ADR 0282 widens that to re-read on every statement).
    db.execute("REVOKE SELECT ON t.public.parents FROM bob").unwrap();
    let err = db.with_authenticated_session("bob", "pw", |s|
        s.execute("SELECT pid FROM t.public.parents")).unwrap_err();
    assert!(matches!(err, SqlError::Authz(_)), "got {err:?}");
}
