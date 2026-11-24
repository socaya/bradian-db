//! ADR 0296 — SQL text → SqlLogicalNode tree → execute parity.
//!
//! For each of three SQL statements against the same seeded
//! table, the hand-built `SqlLogicalNode` plan executed over
//! the table rows produces the same row set the direct
//! `SqlEngine::execute` path produces. This is the plan-tree
//! parity contract — the SQL engine is the oracle; the plan
//! tree is the staging point future slices will decorate.
//!
//! Automatic SQL → plan compilation is a follow-up (noted in
//! ADR 0296 Decision).

use bradian_core::{Column as CoreColumn, LogicalType, Schema as CoreSchema, Value as CoreValue};
use bradian_planner::sql_plan::{
    execute, FilterPredicate, SqlLogicalNode, SqlRowSet,
};
use bradian_relational::sql::{SqlEngine, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed() -> (tempfile::TempDir, SqlEngine) {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, qty BIGINT, label TEXT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, qty, label) VALUES(1, 10, 'alpha')",
        "INSERT INTO t.public.items(id, qty, label) VALUES(2, 20, 'beta')",
        "INSERT INTO t.public.items(id, qty, label) VALUES(3, 30, NULL)",
        "INSERT INTO t.public.items(id, qty, label) VALUES(4, 15, 'alpha')",
    ] { db.execute(sql).unwrap(); }
    (dir, db)
}

fn core_schema() -> CoreSchema {
    CoreSchema::new(vec![
        CoreColumn::new("id",    LogicalType::I64).required(),
        CoreColumn::new("qty",   LogicalType::I64),
        CoreColumn::new("label", LogicalType::Utf8),
    ]).unwrap()
}

fn to_core(v: &Value) -> CoreValue {
    match v {
        Value::Null            => CoreValue::Null,
        Value::Int64(x)        => CoreValue::I64(*x),
        Value::String(s)       => CoreValue::Str(s.clone()),
        other => panic!("ADR 0296 slice covers Int64/Text/Null only; got {other:?}"),
    }
}

fn direct_rows(db: &SqlEngine, sql: &str) -> Vec<Vec<CoreValue>> {
    match db.execute(sql).unwrap() {
        SqlResultSet::Rows { rows, .. } => rows.into_iter()
            .map(|row| row.iter().map(to_core).collect()).collect(),
        other => panic!("direct SQL did not return rows: {other:?}"),
    }
}

/// Load the full table rows via `SELECT *` so the plan tree
/// has a Scan source to work from.
fn full_rows(db: &SqlEngine) -> Vec<Vec<CoreValue>> {
    // ORDER BY id so both paths produce stable, comparable
    // row orders. The SQL engine's row path returns rows
    // sorted by encoded PK (id) already, which matches this.
    let rs = db.execute("SELECT id, qty, label FROM t.public.items").unwrap();
    match rs {
        SqlResultSet::Rows { rows, .. } => {
            let mut rows: Vec<Vec<CoreValue>> = rows.into_iter()
                .map(|row| row.iter().map(to_core).collect()).collect();
            rows.sort_by(|a, b| a[0].cmp(&b[0]));
            rows
        }
        other => panic!("{other:?}"),
    }
}

fn sort_by_id(mut rows: Vec<Vec<CoreValue>>) -> Vec<Vec<CoreValue>> {
    rows.sort_by(|a, b| a[0].cmp(&b[0]));
    rows
}

#[test]
fn plan_tree_matches_sql_engine_for_narrow_subset() {
    let (_dir, db) = seed();
    let schema = core_schema();

    // Case 1: SELECT * FROM items.
    //
    // Plan: Scan.
    let full = full_rows(&db);
    let node = SqlLogicalNode::Scan {
        source: "t.public.items".into(),
        column_names: vec!["id".into(), "qty".into(), "label".into()],
    };
    let plan_out = execute(&node, SqlRowSet { schema: &schema, rows: &full }).unwrap();
    assert_eq!(sort_by_id(plan_out.rows.clone()),
               sort_by_id(direct_rows(&db,
                   "SELECT id, qty, label FROM t.public.items")));

    // Case 2: SELECT id, qty FROM items WHERE label = 'alpha'.
    //
    // Plan: Project ← Filter(label = 'alpha') ← Scan.
    let node = SqlLogicalNode::Project {
        input: Box::new(SqlLogicalNode::Filter {
            input: Box::new(SqlLogicalNode::Scan {
                source: "t.public.items".into(),
                column_names: vec!["id".into(), "qty".into(), "label".into()],
            }),
            predicate: FilterPredicate::Eq {
                column: "label".into(),
                literal: CoreValue::Str("alpha".into()),
            },
        }),
        keep: vec!["id".into(), "qty".into()],
    };
    let plan_out = execute(&node, SqlRowSet { schema: &schema, rows: &full }).unwrap();
    // Direct path: SqlEngine does not have ORDER BY on this
    // subset, so compare as a sorted set.
    let direct = direct_rows(&db,
        "SELECT id, qty FROM t.public.items WHERE label = 'alpha'");
    assert_eq!(sort_by_id(plan_out.rows), sort_by_id(direct));

    // Case 3: SELECT id FROM items WHERE qty > 15.
    let node = SqlLogicalNode::Project {
        input: Box::new(SqlLogicalNode::Filter {
            input: Box::new(SqlLogicalNode::Scan {
                source: "t.public.items".into(),
                column_names: vec!["id".into(), "qty".into(), "label".into()],
            }),
            predicate: FilterPredicate::Gt {
                column: "qty".into(),
                literal: CoreValue::I64(15),
            },
        }),
        keep: vec!["id".into()],
    };
    let plan_out = execute(&node, SqlRowSet { schema: &schema, rows: &full }).unwrap();
    let direct = direct_rows(&db,
        "SELECT id FROM t.public.items WHERE qty > 15");
    assert_eq!(sort_by_id(plan_out.rows), sort_by_id(direct));
}
