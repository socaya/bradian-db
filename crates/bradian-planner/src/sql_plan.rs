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

use std::collections::HashMap;

use bradian_core::{BradianError, BradianResult, Column, Schema, Value};

/// A node in the narrow SQL logical plan tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SqlLogicalNode {
    Scan      { source: String, column_names: Vec<String> },
    Filter    { input: Box<SqlLogicalNode>, predicate: FilterPredicate },
    Project   { input: Box<SqlLogicalNode>, keep: Vec<String> },
    /// ADR 0298 — inner equi-join. Build-side is `left`.
    Join      {
        left:  Box<SqlLogicalNode>,
        right: Box<SqlLogicalNode>,
        on:    JoinOn,
    },
}

/// Equi-join condition: `left.left_column = right.right_column`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinOn {
    pub left_column:  String,
    pub right_column: String,
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

/// ADR 0298 — a bag of named row sets so the interpreter
/// can resolve more than one `Scan` source in the same
/// plan (needed for Join).
pub struct SqlInputs<'a> {
    pub sources: HashMap<String, SqlRowSet<'a>>,
}

impl<'a> SqlInputs<'a> {
    pub fn single(name: impl Into<String>, set: SqlRowSet<'a>) -> Self {
        let mut sources = HashMap::new();
        sources.insert(name.into(), set);
        Self { sources }
    }

    pub fn with(mut self, name: impl Into<String>, set: SqlRowSet<'a>) -> Self {
        self.sources.insert(name.into(), set);
        self
    }
}

/// Executed output: owned rows + the output schema after
/// any projection.
#[derive(Debug, PartialEq)]
pub struct SqlExecOutput {
    pub schema: Schema,
    pub rows:   Vec<Vec<Value>>,
}

/// Bottom-up interpreter. The caller provides a bag of
/// named row sets; each Scan resolves to one of them by
/// source name.
pub fn execute(node: &SqlLogicalNode, inputs: &SqlInputs<'_>) -> BradianResult<SqlExecOutput> {
    match node {
        SqlLogicalNode::Scan { source, column_names } => {
            let set = inputs.sources.get(source).ok_or_else(||
                BradianError::schema_mismatch(format!("unknown Scan source: {source}")))?;
            for c in column_names { set.schema.index_of(c)?; }
            Ok(SqlExecOutput {
                schema: set.schema.clone(),
                rows:   set.rows.to_vec(),
            })
        }
        SqlLogicalNode::Filter { input: child, predicate } => {
            let child = execute(child, inputs)?;
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
            let child = execute(child, inputs)?;
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
        SqlLogicalNode::Join { left, right, on } => {
            let l = execute(left,  inputs)?;
            let r = execute(right, inputs)?;
            execute_inner_equi_join(&l, &r, on)
        }
    }
}

/// Inner equi-join of two pre-executed inputs. Build-side is
/// the left input; probe-side is the right. The output schema
/// concatenates the two input schemas (column names are kept
/// as-is — no qualifier rewrite yet; a duplicate column name
/// errors at Schema::new).
fn execute_inner_equi_join(
    left:  &SqlExecOutput,
    right: &SqlExecOutput,
    on:    &JoinOn,
) -> BradianResult<SqlExecOutput> {
    let li = left.schema.index_of(&on.left_column)?;
    let ri = right.schema.index_of(&on.right_column)?;
    let mut build: HashMap<Value, Vec<&Vec<Value>>> = HashMap::new();
    for row in &left.rows {
        if row[li].is_null() { continue; }
        build.entry(row[li].clone()).or_default().push(row);
    }
    let mut out: Vec<Vec<Value>> = Vec::new();
    for probe in &right.rows {
        if probe[ri].is_null() { continue; }
        if let Some(matches) = build.get(&probe[ri]) {
            for l_row in matches {
                let mut combined = Vec::with_capacity(l_row.len() + probe.len());
                combined.extend(l_row.iter().cloned());
                combined.extend(probe.iter().cloned());
                out.push(combined);
            }
        }
    }
    let combined_cols: Vec<Column> = left.schema.columns.iter()
        .cloned()
        .chain(right.schema.columns.iter().cloned())
        .collect();
    let schema = Schema::new(combined_cols)?;
    Ok(SqlExecOutput { schema, rows: out })
}

/// Row-count estimate used by the join reorder. Scans look
/// up their source's row count in `stats`; Filter applies a
/// static 0.5 selectivity; Project is identity; Join returns
/// `max(left, right)` as a conservative build-side upper
/// bound.
pub fn estimated_rows(node: &SqlLogicalNode, stats: &HashMap<String, usize>) -> usize {
    match node {
        SqlLogicalNode::Scan { source, .. }     => *stats.get(source).unwrap_or(&0),
        SqlLogicalNode::Filter { input, .. }    => estimated_rows(input, stats) / 2,
        SqlLogicalNode::Project { input, .. }   => estimated_rows(input, stats),
        SqlLogicalNode::Join { left, right, .. } => {
            estimated_rows(left, stats).max(estimated_rows(right, stats))
        }
    }
}

/// Bottom-up rewrite: for every Join, if the right side's
/// estimated rows is strictly smaller than the left's, swap
/// the two children and the on-condition. Inner equi-join is
/// commutative so the swap is semantics-preserving.
pub fn reorder_joins(node: &SqlLogicalNode, stats: &HashMap<String, usize>) -> SqlLogicalNode {
    match node {
        SqlLogicalNode::Scan { .. } => node.clone(),
        SqlLogicalNode::Filter { input, predicate } => SqlLogicalNode::Filter {
            input: Box::new(reorder_joins(input, stats)),
            predicate: predicate.clone(),
        },
        SqlLogicalNode::Project { input, keep } => SqlLogicalNode::Project {
            input: Box::new(reorder_joins(input, stats)),
            keep: keep.clone(),
        },
        SqlLogicalNode::Join { left, right, on } => {
            let new_left  = reorder_joins(left,  stats);
            let new_right = reorder_joins(right, stats);
            let l_rows = estimated_rows(&new_left,  stats);
            let r_rows = estimated_rows(&new_right, stats);
            if r_rows < l_rows {
                SqlLogicalNode::Join {
                    left:  Box::new(new_right),
                    right: Box::new(new_left),
                    on: JoinOn {
                        left_column:  on.right_column.clone(),
                        right_column: on.left_column.clone(),
                    },
                }
            } else {
                SqlLogicalNode::Join {
                    left:  Box::new(new_left),
                    right: Box::new(new_right),
                    on:    on.clone(),
                }
            }
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

/// ADR 0297 — structural EXPLAIN node derived from a
/// `SqlLogicalNode`. No text templating; deterministic
/// `Display` output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SqlExplainNode {
    pub operator: String,
    pub details:  Vec<(&'static str, String)>,
    pub children: Vec<SqlExplainNode>,
}

pub fn explain(node: &SqlLogicalNode) -> SqlExplainNode {
    match node {
        SqlLogicalNode::Scan { source, column_names } => SqlExplainNode {
            operator: "Scan".into(),
            details:  vec![
                ("source",  source.clone()),
                ("columns", column_names.join(", ")),
            ],
            children: vec![],
        },
        SqlLogicalNode::Filter { input, predicate } => {
            let (op, column, literal) = match predicate {
                FilterPredicate::Eq { column, literal } => ("=",  column, literal),
                FilterPredicate::Gt { column, literal } => (">",  column, literal),
                FilterPredicate::Lt { column, literal } => ("<",  column, literal),
            };
            SqlExplainNode {
                operator: "Filter".into(),
                details:  vec![
                    ("predicate", format!("{column} {op} {literal:?}")),
                ],
                children: vec![explain(input)],
            }
        }
        SqlLogicalNode::Project { input, keep } => SqlExplainNode {
            operator: "Project".into(),
            details:  vec![("columns", keep.join(", "))],
            children: vec![explain(input)],
        },
        SqlLogicalNode::Join { left, right, on } => SqlExplainNode {
            operator: "Join".into(),
            details:  vec![("on",
                format!("{} = {}", on.left_column, on.right_column))],
            children: vec![explain(left), explain(right)],
        },
    }
}

impl std::fmt::Display for SqlExplainNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn render(node: &SqlExplainNode, depth: usize,
                  f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let pad = "  ".repeat(depth);
            writeln!(f, "{pad}{}", node.operator)?;
            for (k, v) in &node.details {
                writeln!(f, "{pad}  {k}: {v}")?;
            }
            for child in &node.children { render(child, depth + 1, f)?; }
            Ok(())
        }
        render(self, 0, f)
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
        let out = execute(&node, &SqlInputs::single("items", SqlRowSet { schema: &s, rows: &r })).unwrap();
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
        let out = execute(&node, &SqlInputs::single("items", SqlRowSet { schema: &s, rows: &r })).unwrap();
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
        let out = execute(&node, &SqlInputs::single("items", SqlRowSet { schema: &s, rows: &r })).unwrap();
        assert_eq!(out.schema.len(), 2);
        assert_eq!(out.schema.columns[0].name, "label");
        assert_eq!(out.schema.columns[1].name, "id");
        assert_eq!(out.rows[0], vec![Value::Str("alpha".into()), Value::I64(1)]);
    }

    fn join_sources<'a>(
        emp_schema:  &'a Schema, emp_rows:  &'a [Vec<Value>],
        dept_schema: &'a Schema, dept_rows: &'a [Vec<Value>],
    ) -> SqlInputs<'a> {
        SqlInputs::single("emp", SqlRowSet { schema: emp_schema,  rows: emp_rows })
            .with("dept",        SqlRowSet { schema: dept_schema, rows: dept_rows })
    }

    fn emp_schema() -> Schema {
        Schema::new(vec![
            Column::new("emp_id",  LogicalType::I64).required(),
            Column::new("dept_id", LogicalType::I64).required(),
        ]).unwrap()
    }
    fn dept_schema() -> Schema {
        Schema::new(vec![
            Column::new("d_id",   LogicalType::I64).required(),
            Column::new("d_name", LogicalType::Utf8),
        ]).unwrap()
    }

    #[test]
    fn inner_equi_join_builds_on_left_and_probes_on_right() {
        let es = emp_schema();
        let er: Vec<Vec<Value>> = (0..10)
            .map(|i| vec![Value::I64(i), Value::I64(i % 3)]).collect();
        let ds = dept_schema();
        let dr: Vec<Vec<Value>> = (0..3)
            .map(|i| vec![Value::I64(i), Value::Str(format!("d{i}"))]).collect();
        let inputs = join_sources(&es, &er, &ds, &dr);
        let node = SqlLogicalNode::Join {
            left:  Box::new(SqlLogicalNode::Scan {
                source: "emp".into(),
                column_names: vec!["emp_id".into(), "dept_id".into()],
            }),
            right: Box::new(SqlLogicalNode::Scan {
                source: "dept".into(),
                column_names: vec!["d_id".into(), "d_name".into()],
            }),
            on: JoinOn { left_column: "dept_id".into(), right_column: "d_id".into() },
        };
        let out = execute(&node, &inputs).unwrap();
        assert_eq!(out.schema.len(), 4);
        // 10 employees, 3 departments → every employee matches one dept.
        assert_eq!(out.rows.len(), 10);
    }

    #[test]
    fn reorder_swaps_when_right_is_smaller_and_preserves_semantics() {
        let node = SqlLogicalNode::Join {
            left:  Box::new(SqlLogicalNode::Scan {
                source: "big".into(), column_names: vec!["k".into()],
            }),
            right: Box::new(SqlLogicalNode::Scan {
                source: "small".into(), column_names: vec!["k".into()],
            }),
            on: JoinOn { left_column: "k".into(), right_column: "k".into() },
        };
        let stats: HashMap<String, usize> = [
            ("big".into(),   10_000usize),
            ("small".into(), 10usize),
        ].into_iter().collect();
        let reordered = reorder_joins(&node, &stats);
        match &reordered {
            SqlLogicalNode::Join { left, right, on } => {
                // The smaller source must now be on the LEFT.
                match (&**left, &**right) {
                    (SqlLogicalNode::Scan { source: l, .. },
                     SqlLogicalNode::Scan { source: r, .. }) => {
                        assert_eq!(l, "small");
                        assert_eq!(r, "big");
                    }
                    _ => panic!("expected two Scans"),
                }
                // On-condition still matches semantically — both
                // columns are named `k`, so no visible change.
                assert_eq!(on.left_column,  "k");
                assert_eq!(on.right_column, "k");
            }
            _ => panic!("reorder dropped the Join"),
        }
    }

    #[test]
    fn reorder_rewrites_chained_three_way_join_so_build_sides_shrink() {
        // Plan: (big ⋈ medium) ⋈ small on shared key `k`.
        let inner = SqlLogicalNode::Join {
            left:  Box::new(SqlLogicalNode::Scan {
                source: "big".into(), column_names: vec!["k".into()],
            }),
            right: Box::new(SqlLogicalNode::Scan {
                source: "medium".into(), column_names: vec!["k".into()],
            }),
            on: JoinOn { left_column: "k".into(), right_column: "k".into() },
        };
        let outer = SqlLogicalNode::Join {
            left:  Box::new(inner),
            right: Box::new(SqlLogicalNode::Scan {
                source: "small".into(), column_names: vec!["k".into()],
            }),
            on: JoinOn { left_column: "k".into(), right_column: "k".into() },
        };
        let stats: HashMap<String, usize> = [
            ("big".into(),   10_000usize),
            ("medium".into(),  1_000usize),
            ("small".into(),       5usize),
        ].into_iter().collect();
        let reordered = reorder_joins(&outer, &stats);
        // After the outer swap: left=small, right=original_inner.
        match reordered {
            SqlLogicalNode::Join { left, right, .. } => {
                match *left {
                    SqlLogicalNode::Scan { source, .. } => assert_eq!(source, "small"),
                    _ => panic!("outer left should be the smallest source"),
                }
                // The inner Join should itself have been
                // rewritten: medium on the left, big on the
                // right (medium < big).
                match *right {
                    SqlLogicalNode::Join { left: il, right: ir, .. } => {
                        match (*il, *ir) {
                            (SqlLogicalNode::Scan { source: l, .. },
                             SqlLogicalNode::Scan { source: r, .. }) => {
                                assert_eq!(l, "medium");
                                assert_eq!(r, "big");
                            }
                            _ => panic!("inner Join should hold two Scans after reorder"),
                        }
                    }
                    _ => panic!("outer right should be the inner Join"),
                }
            }
            _ => panic!("reorder dropped the outer Join"),
        }
    }

    #[test]
    fn join_reorder_strictly_reduces_build_side_hash_work_on_asymmetric_input() {
        // Big left, tiny right, hash-build-cost proxy = size
        // of whatever ends up on the left. If reorder picks
        // the smaller side as the build (left), build cost
        // drops strictly.
        let es = emp_schema();
        let er: Vec<Vec<Value>> = (0..1000)
            .map(|i| vec![Value::I64(i), Value::I64(i % 3)]).collect();
        let ds = dept_schema();
        let dr: Vec<Vec<Value>> = (0..3)
            .map(|i| vec![Value::I64(i), Value::Str(format!("d{i}"))]).collect();
        let inputs = join_sources(&es, &er, &ds, &dr);
        let unreordered = SqlLogicalNode::Join {
            left:  Box::new(SqlLogicalNode::Scan {
                source: "emp".into(),
                column_names: vec!["emp_id".into(), "dept_id".into()],
            }),
            right: Box::new(SqlLogicalNode::Scan {
                source: "dept".into(),
                column_names: vec!["d_id".into(), "d_name".into()],
            }),
            on: JoinOn { left_column: "dept_id".into(), right_column: "d_id".into() },
        };
        let stats: HashMap<String, usize> = [
            ("emp".into(),  1000usize),
            ("dept".into(),    3usize),
        ].into_iter().collect();
        let reordered = reorder_joins(&unreordered, &stats);

        fn build_side_size(node: &SqlLogicalNode, inputs: &SqlInputs<'_>) -> usize {
            match node {
                SqlLogicalNode::Join { left, .. } => execute(left, inputs).unwrap().rows.len(),
                _ => 0,
            }
        }
        assert!(build_side_size(&reordered, &inputs)
              < build_side_size(&unreordered, &inputs),
            "reorder did not reduce build-side size");

        // Output cardinality is the same regardless of order
        // (inner equi-join is commutative).
        let a = execute(&unreordered, &inputs).unwrap();
        let b = execute(&reordered,   &inputs).unwrap();
        assert_eq!(a.rows.len(), b.rows.len());
    }

    #[test]
    fn explain_single_scan_node() {
        let node = SqlLogicalNode::Scan {
            source: "t.public.items".into(),
            column_names: vec!["id".into(), "qty".into()],
        };
        let ex = explain(&node);
        assert_eq!(ex.operator, "Scan");
        assert!(ex.children.is_empty());
        assert_eq!(format!("{ex}"),
            "Scan\n  source: t.public.items\n  columns: id, qty\n");
    }

    #[test]
    fn explain_filter_scan_tree_is_two_levels() {
        let node = SqlLogicalNode::Filter {
            input: Box::new(SqlLogicalNode::Scan {
                source: "t.public.items".into(),
                column_names: vec!["id".into(), "qty".into()],
            }),
            predicate: FilterPredicate::Eq {
                column: "id".into(),
                literal: Value::I64(7),
            },
        };
        let ex = explain(&node);
        assert_eq!(ex.operator, "Filter");
        assert_eq!(ex.children.len(), 1);
        assert_eq!(ex.children[0].operator, "Scan");
        let display = format!("{ex}");
        assert!(display.contains("Filter"));
        assert!(display.contains("predicate: id = I64(7)"));
        assert!(display.contains("  Scan"),
            "expected nested Scan with two-space indent, got:\n{display}");
    }

    #[test]
    fn explain_project_filter_scan_renders_three_indented_levels() {
        let node = SqlLogicalNode::Project {
            input: Box::new(SqlLogicalNode::Filter {
                input: Box::new(SqlLogicalNode::Scan {
                    source: "t.public.items".into(),
                    column_names: vec!["id".into(), "qty".into(), "label".into()],
                }),
                predicate: FilterPredicate::Gt {
                    column: "qty".into(),
                    literal: Value::I64(15),
                },
            }),
            keep: vec!["id".into()],
        };
        let ex = explain(&node);
        let display = format!("{ex}");
        let expected = "\
Project
  columns: id
  Filter
    predicate: qty > I64(15)
    Scan
      source: t.public.items
      columns: id, qty, label
";
        assert_eq!(display, expected);
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
        let out = execute(&node, &SqlInputs::single("items", SqlRowSet { schema: &s, rows: &r })).unwrap();
        // qty > 15 → rows 2, 3; project id → [2, 3].
        let ids: Vec<i64> = out.rows.iter().map(|row| match row[0] {
            Value::I64(x) => x, _ => panic!("expected i64"),
        }).collect();
        assert_eq!(ids, vec![2, 3]);
    }
}
