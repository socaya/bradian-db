//! ADR 0184 — FOREIGN KEY RESTRICT enforcement in `SqlEngine`.

use bradian_relational::sql::{SqlEngine, SqlError};
use tempfile::tempdir;

fn open(dir: &std::path::Path) -> SqlEngine { SqlEngine::open(dir).unwrap() }

fn seed(db: &SqlEngine) {
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.parents (id BIGINT NOT NULL, label TEXT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.children (id BIGINT NOT NULL, parent_id BIGINT NOT NULL, FOREIGN KEY (parent_id) REFERENCES t.public.parents (id), PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
}

#[test]
fn child_insert_with_existing_parent_succeeds() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seed(&db);
    db.execute("INSERT INTO t.public.parents(id, label) VALUES(1, 'one')").unwrap();
    db.execute("INSERT INTO t.public.children(id, parent_id) VALUES(10, 1)").unwrap();
}

#[test]
fn child_insert_without_parent_is_rejected() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seed(&db);
    let err = db.execute(
        "INSERT INTO t.public.children(id, parent_id) VALUES(10, 99)",
    ).unwrap_err();
    match err {
        SqlError::ForeignKey { constraint, reason } => {
            assert!(constraint.starts_with("fk_children"));
            assert_eq!(reason, "missing-parent");
        }
        other => panic!("expected SqlError::ForeignKey; got {other:?}"),
    }
}

#[test]
fn child_update_fk_column_to_missing_parent_is_rejected() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seed(&db);
    db.execute("INSERT INTO t.public.parents(id, label) VALUES(1, 'one')").unwrap();
    db.execute("INSERT INTO t.public.children(id, parent_id) VALUES(10, 1)").unwrap();
    let err = db.execute(
        "UPDATE t.public.children SET parent_id=99 WHERE id=10",
    ).unwrap_err();
    assert!(matches!(err, SqlError::ForeignKey { reason, .. } if reason == "missing-parent"));
}

#[test]
fn parent_delete_is_restricted_when_children_exist() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seed(&db);
    db.execute("INSERT INTO t.public.parents(id, label) VALUES(1, 'one')").unwrap();
    db.execute("INSERT INTO t.public.children(id, parent_id) VALUES(10, 1)").unwrap();
    let err = db.execute(
        "DELETE FROM t.public.parents WHERE id=1",
    ).unwrap_err();
    assert!(matches!(err, SqlError::ForeignKey { reason, .. } if reason == "restrict"));
}

#[test]
fn parent_delete_after_children_removed_succeeds() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seed(&db);
    db.execute("INSERT INTO t.public.parents(id, label) VALUES(1, 'one')").unwrap();
    db.execute("INSERT INTO t.public.children(id, parent_id) VALUES(10, 1)").unwrap();
    db.execute("DELETE FROM t.public.children WHERE id=10").unwrap();
    // No children left — parent can now be deleted.
    db.execute("DELETE FROM t.public.parents WHERE id=1").unwrap();
}

#[test]
fn nullable_fk_column_null_disables_check() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.parents (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.children (id BIGINT NOT NULL, parent_id BIGINT, FOREIGN KEY (parent_id) REFERENCES t.public.parents (id), PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    // parent_id NULL disables the FK for the row.
    db.execute("INSERT INTO t.public.children(id, parent_id) VALUES(10, NULL)").unwrap();
    // Setting an existing parent still works.
    db.execute("INSERT INTO t.public.parents(id) VALUES(1)").unwrap();
    db.execute("INSERT INTO t.public.children(id, parent_id) VALUES(11, 1)").unwrap();
}

#[test]
fn composite_fk_rejects_partial_match() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.p (a BIGINT NOT NULL, b BIGINT NOT NULL, PRIMARY KEY(a, b))",
        "CREATE TABLE t.public.c (id BIGINT NOT NULL, pa BIGINT NOT NULL, pb BIGINT NOT NULL, FOREIGN KEY (pa, pb) REFERENCES t.public.p (a, b), PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    db.execute("INSERT INTO t.public.p(a, b) VALUES(1, 2)").unwrap();
    db.execute("INSERT INTO t.public.p(a, b) VALUES(3, 4)").unwrap();
    db.execute("INSERT INTO t.public.c(id, pa, pb) VALUES(10, 1, 2)").unwrap();
    // (1, 4) is not a parent tuple even though 1 and 4 both
    // appear on their own in parents.
    let err = db.execute(
        "INSERT INTO t.public.c(id, pa, pb) VALUES(11, 1, 4)",
    ).unwrap_err();
    assert!(matches!(err, SqlError::ForeignKey { reason, .. } if reason == "missing-parent"));
}

#[test]
fn foreign_key_persists_across_restart() {
    let dir = tempdir().unwrap();
    {
        let db = open(dir.path());
        seed(&db);
        db.execute("INSERT INTO t.public.parents(id, label) VALUES(1, 'one')").unwrap();
        db.execute("INSERT INTO t.public.children(id, parent_id) VALUES(10, 1)").unwrap();
    }
    let db = open(dir.path());
    // RESTRICT still fires after reopen.
    let err = db.execute("DELETE FROM t.public.parents WHERE id=1").unwrap_err();
    assert!(matches!(err, SqlError::ForeignKey { reason, .. } if reason == "restrict"));
    // Missing-parent still fires.
    let err = db.execute(
        "INSERT INTO t.public.children(id, parent_id) VALUES(11, 99)"
    ).unwrap_err();
    assert!(matches!(err, SqlError::ForeignKey { reason, .. } if reason == "missing-parent"));
}

#[test]
fn foreign_key_referencing_missing_parent_name_is_rejected_at_create() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
    ] { db.execute(sql).unwrap(); }
    let err = db.execute(
        "CREATE TABLE t.public.c (id BIGINT NOT NULL, p BIGINT NOT NULL, FOREIGN KEY (p) REFERENCES t.public.missing (id), PRIMARY KEY(id))"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Resolution(_)),
        "unknown parent must fail cleanly at CREATE; got {err:?}");
}

#[test]
fn foreign_key_arity_mismatch_is_rejected_at_create() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.p (a BIGINT NOT NULL, b BIGINT NOT NULL, PRIMARY KEY(a, b))",
    ] { db.execute(sql).unwrap(); }
    let err = db.execute(
        "CREATE TABLE t.public.c (id BIGINT NOT NULL, pa BIGINT NOT NULL, FOREIGN KEY (pa) REFERENCES t.public.p (a, b), PRIMARY KEY(id))"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Catalog(_)),
        "FK arity mismatch must be caught by catalog; got {err:?}");
}
