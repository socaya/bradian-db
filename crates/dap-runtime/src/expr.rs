//! Typed row expressions evaluated by the scalar oracle AND by the vectorized filter kernel.
//!
//! v1 focus: equality + comparison + logical combinations over column references and literals.
//! The planner compiles DHIS2 queries down to these primitives. Property tests assert that
//! `vector_eval == scalar_eval` over random data.

use dap_core::{DapError, DapResult, Value};
use serde::{Deserialize, Serialize};

use crate::batch::RecordBatch;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Expr {
    /// Reference to a schema column by name.
    Col(String),
    /// A constant literal.
    Lit(Value),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CmpOp {
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Predicate {
    True,
    False,
    Compare {
        left: Expr,
        op: CmpOp,
        right: Expr,
    },
    And(Vec<Predicate>),
    Or(Vec<Predicate>),
    Not(Box<Predicate>),
    /// Column IS NULL / IS NOT NULL.
    IsNull(Expr),
    IsNotNull(Expr),
    /// `expr IN (value, ...)` — compiled from the semantic DSL.
    In { expr: Expr, values: Vec<Value> },
}

impl Predicate {
    /// Scalar evaluation for a single row. Used by the oracle AND the fallback path for
    /// predicates the vectorized kernel does not accelerate.
    pub fn eval_row(&self, batch: &RecordBatch, row: usize) -> DapResult<bool> {
        match self {
            Predicate::True => Ok(true),
            Predicate::False => Ok(false),
            Predicate::And(ps) => {
                for p in ps {
                    if !p.eval_row(batch, row)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Predicate::Or(ps) => {
                for p in ps {
                    if p.eval_row(batch, row)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Predicate::Not(p) => Ok(!p.eval_row(batch, row)?),
            Predicate::IsNull(e) => Ok(e.eval(batch, row)?.is_null()),
            Predicate::IsNotNull(e) => Ok(!e.eval(batch, row)?.is_null()),
            Predicate::Compare { left, op, right } => {
                let lv = left.eval(batch, row)?;
                let rv = right.eval(batch, row)?;
                // Null comparisons yield false per SQL three-valued logic flattened to boolean.
                if lv.is_null() || rv.is_null() {
                    return Ok(false);
                }
                let ord = lv.cmp(&rv);
                Ok(match op {
                    CmpOp::Eq => ord.is_eq(),
                    CmpOp::NotEq => ord.is_ne(),
                    CmpOp::Lt => ord.is_lt(),
                    CmpOp::LtEq => ord.is_le(),
                    CmpOp::Gt => ord.is_gt(),
                    CmpOp::GtEq => ord.is_ge(),
                })
            }
            Predicate::In { expr, values } => {
                let v = expr.eval(batch, row)?;
                if v.is_null() {
                    return Ok(false);
                }
                Ok(values.iter().any(|needle| needle == &v))
            }
        }
    }

    /// Evaluate the predicate across every row of the batch, returning a selection vector.
    pub fn eval_batch(&self, batch: &RecordBatch) -> DapResult<Vec<u32>> {
        let mut out = Vec::new();
        for r in 0..batch.row_count() {
            if self.eval_row(batch, r)? {
                out.push(r as u32);
            }
        }
        Ok(out)
    }
}

impl Expr {
    pub fn eval(&self, batch: &RecordBatch, row: usize) -> DapResult<Value> {
        match self {
            Expr::Lit(v) => Ok(v.clone()),
            Expr::Col(name) => {
                let col = batch.column(name).map_err(|_| {
                    DapError::NotFound(format!("column {name} in expression"))
                })?;
                col.value_at(row)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dap_core::{Column, LogicalType, Schema};
    use dap_storage::ColumnBuffer;

    fn mk() -> RecordBatch {
        let schema = Schema::new(vec![
            Column::new("ou", LogicalType::Uid).required(),
            Column::new("v", LogicalType::I64),
        ])
        .unwrap();
        // Build columns manually.
        let mut ou = dap_storage::column::ColumnBuilder::new(LogicalType::Uid);
        let mut v = dap_storage::column::ColumnBuilder::new(LogicalType::I64);
        for (o, x) in [("a", Some(1)), ("b", Some(2)), ("c", None), ("a", Some(5))] {
            ou.append_value(&Value::Str(o.into())).unwrap();
            match x {
                Some(i) => v.append_value(&Value::I64(i)).unwrap(),
                None => v.append_value(&Value::Null).unwrap(),
            }
        }
        RecordBatch::new(schema, vec![ou.finish(), v.finish()]).unwrap()
    }

    #[test]
    fn equality_filter() {
        let batch = mk();
        let pred = Predicate::Compare {
            left: Expr::Col("ou".into()),
            op: CmpOp::Eq,
            right: Expr::Lit(Value::Str("a".into())),
        };
        assert_eq!(pred.eval_batch(&batch).unwrap(), vec![0, 3]);
    }

    #[test]
    fn range_filter() {
        let batch = mk();
        let pred = Predicate::Compare {
            left: Expr::Col("v".into()),
            op: CmpOp::GtEq,
            right: Expr::Lit(Value::I64(2)),
        };
        assert_eq!(pred.eval_batch(&batch).unwrap(), vec![1, 3]);
    }

    #[test]
    fn null_comparison_is_false() {
        let batch = mk();
        let pred = Predicate::Compare {
            left: Expr::Col("v".into()),
            op: CmpOp::Eq,
            right: Expr::Lit(Value::I64(0)),
        };
        let sel = pred.eval_batch(&batch).unwrap();
        assert!(!sel.contains(&2), "null row must not match");
    }

    #[test]
    fn is_null_filter() {
        let batch = mk();
        let pred = Predicate::IsNull(Expr::Col("v".into()));
        assert_eq!(pred.eval_batch(&batch).unwrap(), vec![2]);
    }

    #[test]
    fn in_list() {
        let batch = mk();
        let pred = Predicate::In {
            expr: Expr::Col("ou".into()),
            values: vec![Value::Str("b".into()), Value::Str("c".into())],
        };
        assert_eq!(pred.eval_batch(&batch).unwrap(), vec![1, 2]);
    }

    #[test]
    fn and_or_combinator() {
        let batch = mk();
        let pred = Predicate::And(vec![
            Predicate::Compare {
                left: Expr::Col("ou".into()),
                op: CmpOp::Eq,
                right: Expr::Lit(Value::Str("a".into())),
            },
            Predicate::IsNotNull(Expr::Col("v".into())),
        ]);
        assert_eq!(pred.eval_batch(&batch).unwrap(), vec![0, 3]);
    }

    // Suppress "unused" warning on ColumnBuffer import which is only exercised transitively.
    fn _touch(_c: &ColumnBuffer) {}
}
