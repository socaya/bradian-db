//! ADR 0296 — SQL logical plan tree (narrow subset).
//!
//! A deliberately small set of operators — Scan, Filter,
//! Project — plus an interpreter that applies them
//! bottom-up over an in-memory row set carrying its input
//! schema. The tree is the explicit staging point that
//! future slices (EXPLAIN surface, cost-based join
//! reordering) will decorate; automatic SQL-text → tree
//! compilation is a follow-up.
//!
//! This module is intentionally independent of the
//! DHIS2-oriented `plan::LogicalPlan`: the two inhabit
//! different crates of the query lifecycle (relational vs
//! analytical) and should not share a representation until
//! a cross-cutting SQL compiler exists.

use bradian_core::{BradianResult, Schema, Value};

/// A node in the narrow SQL logical plan tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SqlLogicalNode {
    Scan      { source: String, column_names: Vec<String> },
    Filter    { input: Box<SqlLogicalNode>, predicate: FilterPredicate },
    Project   { input: Box<SqlLogicalNode>, keep: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilterPredicate {
    Eq { column: String, literal: Value },
    Gt { column: String, literal: Value },
    Lt { column: String, literal: Value },
}

/// An input row set the interpreter borrows: rows plus the
/// schema the top-level Scan produced.
pub struct SqlRowSet<'a> {
    pub schema: &'a Schema,
    pub rows:   &'a [Vec<Value>],
}

/// Executed output: owned rows + the output schema after
/// any projection.
#[derive(Debug, PartialEq)]
pub struct SqlExecOutput {
    pub schema: Schema,
    pub rows:   Vec<Vec<Value>>,
}

/// Bottom-up interpreter. The caller provides the raw
/// `(schema, rows)` that the Scan source would yield; the
/// interpreter then applies Filter / Project in order.
pub fn execute(node: &SqlLogicalNode, input: SqlRowSet<'_>) -> BradianResult<SqlExecOutput> {
    match node {
        SqlLogicalNode::Scan { source: _, column_names } => {
            // Validate that the caller's schema actually
            // carries the names the Scan claimed. The source
            // name is a label only (no side-table lookup
            // yet; the rows ARE the source).
            for c in column_names {
                input.schema.index_of(c)?;
            }
            Ok(SqlExecOutput {
                schema: input.schema.clone(),
                rows:   input.rows.to_vec(),
            })
        }
        SqlLogicalNode::Filter { input: child, predicate } => {
            let child = execute(child, input)?;
            let idx = child.schema.index_of(predicate.column())?;
            let mut kept = Vec::with_capacity(child.rows.len());
            for row in &child.rows {
                if predicate.matches(&row[idx]) {
                    kept.push(row.clone());
                }
            }
            Ok(SqlExecOutput { schema: child.schema, rows: kept })
        }
        SqlLogicalNode::Project { input: child, keep } => {
            let child = execute(child, input)?;
            let idxs: Vec<usize> = keep.iter()
                .map(|c| child.schema.index_of(c))
                .collect::<BradianResult<Vec<_>>>()?;
            let projected_cols: Vec<_> = idxs.iter()
                .map(|&i| child.schema.columns[i].clone())
                .collect();
            let projected_schema = Schema::new(projected_cols)?;
            let rows = child.rows.iter()
                .map(|row| idxs.iter().map(|&i| row[i].clone()).collect())
                .collect();
            Ok(SqlExecOutput { schema: projected_schema, rows })
        }
    }
}

impl FilterPredicate {
    pub fn column(&self) -> &str {
        match self {
            FilterPredicate::Eq { column, .. }
            | FilterPredicate::Gt { column, .. }
            | FilterPredicate::Lt { column, .. } => column,
        }
    }

    fn matches(&self, value: &Value) -> bool {
        // SQL three-valued logic flattened to boolean: NULL
        // comparisons are UNKNOWN which filters to false.
        if value.is_null() { return false; }
        match self {
            FilterPredicate::Eq { literal, .. } => value.cmp(literal).is_eq(),
            FilterPredicate::Gt { literal, .. } => value.cmp(literal).is_gt(),
            FilterPredicate::Lt { literal, .. } => value.cmp(literal).is_lt(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bradian_core::{Column, LogicalType};

    fn schema() -> Schema {
        Schema::new(vec![
            Column::new("id",     LogicalType::I64).required(),
            Column::new("qty",    LogicalType::I64),
            Column::new("label",  LogicalType::Utf8),
        ]).unwrap()
    }

    fn rows() -> Vec<Vec<Value>> {
        vec![
            vec![Value::I64(1),  Value::I64(10),   Value::Str("alpha".into())],
            vec![Value::I64(2),  Value::I64(20),   Value::Str("beta".into())],
            vec![Value::I64(3),  Value::I64(30),   Value::Null],
            vec![Value::I64(4),  Value::I64(15),   Value::Str("alpha".into())],
        ]
    }

    #[test]
    fn scan_returns_input_untouched() {
        let s = schema();
        let r = rows();
        let node = SqlLogicalNode::Scan {
            source: "items".into(),
            column_names: vec!["id".into(), "qty".into(), "label".into()],
        };
        let out = execute(&node, SqlRowSet { schema: &s, rows: &r }).unwrap();
        assert_eq!(out.rows, r);
        assert_eq!(out.schema.len(), 3);
    }

    #[test]
    fn filter_eq_on_text_column_filters_rows_and_skips_nulls() {
        let s = schema();
        let r = rows();
        let node = SqlLogicalNode::Filter {
            input: Box::new(SqlLogicalNode::Scan {
                source: "items".into(),
                column_names: vec!["id".into(), "qty".into(), "label".into()],
            }),
            predicate: FilterPredicate::Eq {
                column: "label".into(),
                literal: Value::Str("alpha".into()),
            },
        };
        let out = execute(&node, SqlRowSet { schema: &s, rows: &r }).unwrap();
        let ids: Vec<i64> = out.rows.iter().map(|row| match row[0] {
            Value::I64(x) => x, _ => panic!("expected i64"),
        }).collect();
        assert_eq!(ids, vec![1, 4]);
    }

    #[test]
    fn project_narrows_columns_in_order_and_updates_schema() {
        let s = schema();
        let r = rows();
        let node = SqlLogicalNode::Project {
            input: Box::new(SqlLogicalNode::Scan {
                source: "items".into(),
                column_names: vec!["id".into(), "qty".into(), "label".into()],
            }),
            keep: vec!["label".into(), "id".into()],
        };
        let out = execute(&node, SqlRowSet { schema: &s, rows: &r }).unwrap();
        assert_eq!(out.schema.len(), 2);
        assert_eq!(out.schema.columns[0].name, "label");
        assert_eq!(out.schema.columns[1].name, "id");
        assert_eq!(out.rows[0], vec![Value::Str("alpha".into()), Value::I64(1)]);
    }

    #[test]
    fn filter_then_project_applies_in_tree_order() {
        let s = schema();
        let r = rows();
        let node = SqlLogicalNode::Project {
            input: Box::new(SqlLogicalNode::Filter {
                input: Box::new(SqlLogicalNode::Scan {
                    source: "items".into(),
                    column_names: vec!["id".into(), "qty".into(), "label".into()],
                }),
                predicate: FilterPredicate::Gt {
                    column: "qty".into(),
                    literal: Value::I64(15),
                },
            }),
            keep: vec!["id".into()],
        };
        let out = execute(&node, SqlRowSet { schema: &s, rows: &r }).unwrap();
        // qty > 15 → rows 2, 3; project id → [2, 3].
        let ids: Vec<i64> = out.rows.iter().map(|row| match row[0] {
            Value::I64(x) => x, _ => panic!("expected i64"),
        }).collect();
        assert_eq!(ids, vec![2, 3]);
    }
}
