//! ADR 0185 — ON DELETE action keywords (CASCADE / SET NULL /
//! RESTRICT / NO ACTION) wired through `SqlEngine`.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn open(dir: &std::path::Path) -> SqlEngine { SqlEngine::open(dir).unwrap() }

fn seed_cascade(db: &SqlEngine) {
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.parents (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.children (id BIGINT NOT NULL, parent_id BIGINT NOT NULL, FOREIGN KEY (parent_id) REFERENCES t.public.parents (id) ON DELETE CASCADE, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
}

fn seed_setnull(db: &SqlEngine) {
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.parents (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.children (id BIGINT NOT NULL, parent_id BIGINT, FOREIGN KEY (parent_id) REFERENCES t.public.parents (id) ON DELETE SET NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
}

#[test]
fn on_delete_cascade_removes_matching_children() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seed_cascade(&db);
    db.execute("INSERT INTO t.public.parents(id) VALUES(1)").unwrap();
    db.execute("INSERT INTO t.public.children(id, parent_id) VALUES(10, 1)").unwrap();
    db.execute("INSERT INTO t.public.children(id, parent_id) VALUES(11, 1)").unwrap();
    // Delete the parent; the two children must vanish too.
    db.execute("DELETE FROM t.public.parents WHERE id=1").unwrap();
    match db.execute("SELECT id FROM t.public.children").unwrap() {
        SqlResultSet::Rows { rows, .. } => assert!(rows.is_empty()),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn on_delete_cascade_only_removes_matching_children() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seed_cascade(&db);
    db.execute("INSERT INTO t.public.parents(id) VALUES(1)").unwrap();
    db.execute("INSERT INTO t.public.parents(id) VALUES(2)").unwrap();
    db.execute("INSERT INTO t.public.children(id, parent_id) VALUES(10, 1)").unwrap();
    db.execute("INSERT INTO t.public.children(id, parent_id) VALUES(20, 2)").unwrap();
    db.execute("DELETE FROM t.public.parents WHERE id=1").unwrap();
    match db.execute("SELECT id FROM t.public.children").unwrap() {
        SqlResultSet::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], Value::Int64(20));
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn on_delete_set_null_nulls_the_fk_column() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seed_setnull(&db);
    db.execute("INSERT INTO t.public.parents(id) VALUES(1)").unwrap();
    db.execute("INSERT INTO t.public.children(id, parent_id) VALUES(10, 1)").unwrap();
    db.execute("DELETE FROM t.public.parents WHERE id=1").unwrap();
    match db.execute("SELECT parent_id FROM t.public.children WHERE id=10").unwrap() {
        SqlResultSet::Rows { rows, .. } => assert_eq!(rows, vec![vec![Value::Null]]),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn on_delete_set_null_requires_nullable_child_column_at_create() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.parents (id BIGINT NOT NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    // parent_id is NOT NULL — ON DELETE SET NULL cannot be
    // satisfied. Catalog refuses at CREATE.
    let err = db.execute(
        "CREATE TABLE t.public.children (id BIGINT NOT NULL, parent_id BIGINT NOT NULL, FOREIGN KEY (parent_id) REFERENCES t.public.parents (id) ON DELETE SET NULL, PRIMARY KEY(id))"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Catalog(_)),
        "SET NULL on NOT NULL child must fail at CREATE; got {err:?}");
}

#[test]
fn explicit_on_delete_restrict_matches_default_behavior() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.parents (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.children (id BIGINT NOT NULL, parent_id BIGINT NOT NULL, FOREIGN KEY (parent_id) REFERENCES t.public.parents (id) ON DELETE RESTRICT, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    db.execute("INSERT INTO t.public.parents(id) VALUES(1)").unwrap();
    db.execute("INSERT INTO t.public.children(id, parent_id) VALUES(10, 1)").unwrap();
    let err = db.execute("DELETE FROM t.public.parents WHERE id=1").unwrap_err();
    assert!(matches!(err, SqlError::ForeignKey { reason, .. } if reason == "restrict"));
}

#[test]
fn on_delete_no_action_is_treated_as_restrict() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.parents (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.children (id BIGINT NOT NULL, parent_id BIGINT NOT NULL, FOREIGN KEY (parent_id) REFERENCES t.public.parents (id) ON DELETE NO ACTION, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    db.execute("INSERT INTO t.public.parents(id) VALUES(1)").unwrap();
    db.execute("INSERT INTO t.public.children(id, parent_id) VALUES(10, 1)").unwrap();
    let err = db.execute("DELETE FROM t.public.parents WHERE id=1").unwrap_err();
    assert!(matches!(err, SqlError::ForeignKey { reason, .. } if reason == "restrict"));
}

#[test]
fn cascade_is_transitive_across_fk_chain() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.a (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.b (id BIGINT NOT NULL, a_id BIGINT NOT NULL, FOREIGN KEY (a_id) REFERENCES t.public.a (id) ON DELETE CASCADE, PRIMARY KEY(id))",
        "CREATE TABLE t.public.c (id BIGINT NOT NULL, b_id BIGINT NOT NULL, FOREIGN KEY (b_id) REFERENCES t.public.b (id) ON DELETE CASCADE, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    db.execute("INSERT INTO t.public.a(id) VALUES(1)").unwrap();
    db.execute("INSERT INTO t.public.b(id, a_id) VALUES(10, 1)").unwrap();
    db.execute("INSERT INTO t.public.c(id, b_id) VALUES(100, 10)").unwrap();
    db.execute("DELETE FROM t.public.a WHERE id=1").unwrap();
    for table in ["t.public.a", "t.public.b", "t.public.c"] {
        match db.execute(&format!("SELECT id FROM {table}")).unwrap() {
            SqlResultSet::Rows { rows, .. } => {
                assert!(rows.is_empty(), "table {table} should be empty after cascade");
            }
            other => panic!("expected Rows; got {other:?}"),
        }
    }
}

#[test]
fn cascade_respects_restrict_on_grandchild() {
    // chain: a <- b CASCADE   c <- b RESTRICT (as sibling).
    // Deleting a tries to cascade through b, which triggers
    // RESTRICT on c → the top-level DELETE must fail.
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.a (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.b (id BIGINT NOT NULL, a_id BIGINT NOT NULL, FOREIGN KEY (a_id) REFERENCES t.public.a (id) ON DELETE CASCADE, PRIMARY KEY(id))",
        "CREATE TABLE t.public.c (id BIGINT NOT NULL, b_id BIGINT NOT NULL, FOREIGN KEY (b_id) REFERENCES t.public.b (id) ON DELETE RESTRICT, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    db.execute("INSERT INTO t.public.a(id) VALUES(1)").unwrap();
    db.execute("INSERT INTO t.public.b(id, a_id) VALUES(10, 1)").unwrap();
    db.execute("INSERT INTO t.public.c(id, b_id) VALUES(100, 10)").unwrap();
    let err = db.execute("DELETE FROM t.public.a WHERE id=1").unwrap_err();
    assert!(matches!(err, SqlError::ForeignKey { reason, .. } if reason == "restrict"));
}
