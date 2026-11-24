//! ADR 0270 + 0272 + 0273 — verification tests pinning
//! already-shipped SQL conformance features: IN (SELECT ...)
//! subquery (ADR 0229), INNER JOIN (ADR 0226), and
//! column-in-column WHERE via joined queries.

use bradian_relational::sql::{SqlEngine, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed() -> (tempfile::TempDir, SqlEngine) {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.parents (pid BIGINT NOT NULL, label TEXT, PRIMARY KEY(pid))",
        "CREATE TABLE t.public.children (cid BIGINT NOT NULL, parent BIGINT, score INT, PRIMARY KEY(cid))",
        "INSERT INTO t.public.parents(pid, label) VALUES(1, 'alpha')",
        "INSERT INTO t.public.parents(pid, label) VALUES(2, 'beta')",
        "INSERT INTO t.public.parents(pid, label) VALUES(3, 'gamma')",
        "INSERT INTO t.public.children(cid, parent, score) VALUES(10, 1, 100)",
        "INSERT INTO t.public.children(cid, parent, score) VALUES(11, 2, 200)",
        "INSERT INTO t.public.children(cid, parent, score) VALUES(12, 2, 300)",
        "INSERT INTO t.public.children(cid, parent, score) VALUES(13, 999, 50)",
    ] { db.execute(sql).unwrap(); }
    (dir, db)
}

fn ids(rs: SqlResultSet, col: usize) -> Vec<i64> {
    match rs {
        SqlResultSet::Rows { rows, .. } => rows.into_iter()
            .map(|r| match &r[col] {
                Value::Int64(v) => *v,
                Value::Int32(v) => i64::from(*v),
                other => panic!("non-int cell {other:?}"),
            }).collect(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn in_subquery_filters_rows() {
    let (_dir, db) = seed();
    // children whose parent is in {alpha,beta}'s pids.
    let got = ids(db.execute(
        "SELECT cid FROM t.public.children WHERE parent IN (SELECT pid FROM t.public.parents)"
    ).unwrap(), 0);
    assert_eq!(got, vec![10, 11, 12]);
}

#[test]
fn in_subquery_narrows_by_predicate() {
    // The subquery projects a subset of the outer rows.
    let (_dir, db) = seed();
    let got = ids(db.execute(
        "SELECT cid FROM t.public.children WHERE parent IN (SELECT pid FROM t.public.parents WHERE pid = 1)"
    ).unwrap(), 0);
    assert_eq!(got, vec![10]);
}

#[test]
fn inner_join_on_equality() {
    let (_dir, db) = seed();
    // Join children.parent = parents.pid.
    let rs = db.execute(
        "SELECT c.cid, p.label FROM t.public.children c INNER JOIN t.public.parents p ON c.parent = p.pid"
    ).unwrap();
    match rs {
        SqlResultSet::Rows { rows, .. } => {
            // Row 13 (parent=999) is excluded; others join.
            let cids: Vec<_> = rows.iter().map(|r| match &r[0] {
                Value::Int64(v) => *v, _ => panic!(),
            }).collect();
            assert_eq!(cids.len(), 3);
            for id in &[10, 11, 12] { assert!(cids.contains(id)); }
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn inner_join_filters_non_matching_rows() {
    let (_dir, db) = seed();
    // Only children whose parent matches a row in parents.
    let got = ids(db.execute(
        "SELECT c.cid FROM t.public.children c INNER JOIN t.public.parents p ON c.parent = p.pid"
    ).unwrap(), 0);
    assert_eq!(got.len(), 3);
    assert!(!got.contains(&13));
}
