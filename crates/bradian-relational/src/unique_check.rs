//! ADR 0101 — Secondary unique constraint enforcement utility.
//!
//! Honest scope: this module ships the correctness primitive
//! that a future SQL-layer INSERT/UPDATE path can call to
//! enforce UNIQUE constraints at commit time. The underlying
//! catalog (`Catalog::create_table`) already accepts
//! `unique_keys` and stores them per-table; the gap was
//! enforcement during row mutation.
//!
//! What this module does:
//!   - Given a `Table` with `unique_keys`, a candidate row, and
//!     an iterator over existing rows, report every unique
//!     constraint the candidate would violate.
//!
//! What this module does NOT do:
//!   - Wire itself into the SQL INSERT/UPDATE paths. That
//!     requires per-engine integration (SqlEngine,
//!     DomainSqlEngine, HistorySqlEngine, …).
//!   - Maintain a disk-level secondary index. In-memory scan
//!     only. Disk-backed secondary indexes require the
//!     mutable-storage work on top of ADR 0096 (Gate 3).

use std::collections::BTreeSet;

use crate::model::Table;
use crate::typed_table::Value;

/// A unique-constraint violation detected by
/// `UniqueConstraintChecker`. Carries the offending
/// constraint's name so operators can diagnose at the
/// column-group level.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unique constraint {constraint:?} violated on columns {columns:?}")]
pub struct UniqueViolation {
    pub constraint: String,
    pub columns: Vec<String>,
}

/// Enforces each UNIQUE constraint on a `Table` against a
/// candidate row. Null values participate per SQL-standard
/// treatment: **a null in any unique-key column disables the
/// constraint for that row** (two rows with nulls do not
/// conflict). Operators who need "strictly unique even with
/// nulls" must forbid NULLs with `NOT NULL` first.
#[derive(Debug)]
pub struct UniqueConstraintChecker<'t> {
    table: &'t Table,
}

impl<'t> UniqueConstraintChecker<'t> {
    pub fn new(table: &'t Table) -> Self {
        Self { table }
    }

    /// Row shape must match `table.columns` by position.
    ///
    /// `existing` yields rows already in the table (post-commit
    /// state at scan time). The checker scans each row,
    /// extracts the UNIQUE-key projection, and compares against
    /// the candidate's same projection. First conflict wins.
    pub fn check<I>(&self, candidate: &[Value], existing: I) -> Result<(), UniqueViolation>
    where
        I: IntoIterator<Item = Vec<Value>>,
    {
        if self.table.unique_keys.is_empty() {
            return Ok(());
        }
        let positions: Vec<(String, Vec<String>, Vec<usize>)> =
            self.table.unique_keys.iter().map(|uk| {
                let idxs: Vec<usize> = uk.columns.iter()
                    .filter_map(|col| self.table.columns.iter().position(|c| c.name == *col))
                    .collect();
                (uk.name.clone(), uk.columns.clone(), idxs)
            }).collect();

        // Project the candidate against each unique key; skip
        // keys whose projection contains a NULL (SQL semantics).
        let mut active: Vec<(String, Vec<String>, Vec<Value>)> = Vec::new();
        for (name, cols, idxs) in &positions {
            let proj: Vec<Value> = idxs.iter().map(|i| candidate[*i].clone()).collect();
            if proj.iter().any(|v| matches!(v, Value::Null)) {
                continue;
            }
            active.push((name.clone(), cols.clone(), proj));
        }
        if active.is_empty() {
            return Ok(());
        }

        // Fast-path: collect candidate projections into a set
        // keyed by its bytes.
        let candidate_sets: Vec<(String, Vec<String>, BTreeSet<Vec<u8>>)> =
            active.into_iter().map(|(n, c, proj)| {
                let mut s = BTreeSet::new();
                s.insert(encode_projection(&proj));
                (n, c, s)
            }).collect();

        for row in existing {
            for (name, cols, set) in &candidate_sets {
                let idxs: Vec<usize> = cols.iter()
                    .filter_map(|c| self.table.columns.iter().position(|col| col.name == *c))
                    .collect();
                let proj: Vec<Value> = idxs.iter().map(|i| row[*i].clone()).collect();
                if proj.iter().any(|v| matches!(v, Value::Null)) {
                    continue;
                }
                let key = encode_projection(&proj);
                if set.contains(&key) {
                    return Err(UniqueViolation {
                        constraint: name.clone(),
                        columns: cols.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}

/// Canonicalise a Value tuple into bytes so it can be hashed
/// or compared without re-implementing per-type equality.
fn encode_projection(values: &[Value]) -> Vec<u8> {
    serde_json::to_vec(values).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Column, DataType, UniqueKey};

    fn table(name: &str, cols: &[(&str, DataType, bool)], unique: &[(&str, &[&str])]) -> Table {
        Table {
            id: crate::model::TableId(1),
            schema: crate::model::SchemaId(1),
            name: name.into(),
            columns: cols.iter().map(|(n, t, nullable)| Column {
                name: (*n).into(), data_type: t.clone(), nullable: *nullable,
            }).collect(),
            primary_key: crate::model::PrimaryKey { columns: vec![cols[0].0.into()] },
            unique_keys: unique.iter().map(|(n, c)| UniqueKey {
                name: (*n).into(),
                columns: c.iter().map(|s| (*s).into()).collect(),
            }).collect(),
            foreign_keys: vec![],
            column_defaults: Default::default(),
            column_collations: Default::default(),
            policies: Default::default(),
            column_checks: Default::default(),
        }
    }

    #[test]
    fn no_unique_constraints_always_passes() {
        let t = table("t", &[("id", DataType::Int64, false)], &[]);
        let c = UniqueConstraintChecker::new(&t);
        assert!(c.check(&[Value::Int64(1)], vec![vec![Value::Int64(1)]]).is_ok());
    }

    #[test]
    fn duplicate_on_single_column_unique_is_rejected() {
        let t = table("t",
            &[("id", DataType::Int64, false), ("email", DataType::Text, false)],
            &[("uniq_email", &["email"])]);
        let c = UniqueConstraintChecker::new(&t);
        let existing = vec![vec![Value::Int64(1), Value::String("a@x".into())]];
        let candidate = vec![Value::Int64(2), Value::String("a@x".into())];
        let err = c.check(&candidate, existing).unwrap_err();
        assert_eq!(err.constraint, "uniq_email");
    }

    #[test]
    fn distinct_single_column_passes() {
        let t = table("t",
            &[("id", DataType::Int64, false), ("email", DataType::Text, false)],
            &[("uniq_email", &["email"])]);
        let c = UniqueConstraintChecker::new(&t);
        let existing = vec![vec![Value::Int64(1), Value::String("a@x".into())]];
        let candidate = vec![Value::Int64(2), Value::String("b@x".into())];
        assert!(c.check(&candidate, existing).is_ok());
    }

    #[test]
    fn null_in_unique_key_column_disables_constraint_for_candidate() {
        // SQL-standard semantics: NULL in a UNIQUE column means
        // the constraint does not apply to that row.
        let t = table("t",
            &[("id", DataType::Int64, false), ("email", DataType::Text, true)],
            &[("uniq_email", &["email"])]);
        let c = UniqueConstraintChecker::new(&t);
        let existing = vec![vec![Value::Int64(1), Value::Null]];
        let candidate = vec![Value::Int64(2), Value::Null];
        assert!(c.check(&candidate, existing).is_ok(),
            "two NULLs in a unique column do not conflict");
    }

    #[test]
    fn composite_unique_rejects_full_tuple_duplicate() {
        let t = table("t",
            &[("id", DataType::Int64, false),
              ("tenant", DataType::Int64, false),
              ("name", DataType::Text, false)],
            &[("uniq_tenant_name", &["tenant", "name"])]);
        let c = UniqueConstraintChecker::new(&t);
        let existing = vec![
            vec![Value::Int64(1), Value::Int64(10), Value::String("alice".into())],
            vec![Value::Int64(2), Value::Int64(20), Value::String("bob".into())],
        ];
        // Same name different tenant: ok.
        let ok = vec![Value::Int64(3), Value::Int64(30), Value::String("alice".into())];
        assert!(c.check(&ok, existing.clone()).is_ok());
        // Same (tenant, name): rejected.
        let bad = vec![Value::Int64(4), Value::Int64(10), Value::String("alice".into())];
        let err = c.check(&bad, existing).unwrap_err();
        assert_eq!(err.constraint, "uniq_tenant_name");
        assert_eq!(err.columns, vec!["tenant".to_string(), "name".to_string()]);
    }

    #[test]
    fn multiple_unique_keys_first_violation_wins() {
        let t = table("t",
            &[("id", DataType::Int64, false),
              ("a", DataType::Int64, false), ("b", DataType::Int64, false)],
            &[("uniq_a", &["a"]), ("uniq_b", &["b"])]);
        let c = UniqueConstraintChecker::new(&t);
        let existing = vec![vec![Value::Int64(1), Value::Int64(10), Value::Int64(20)]];
        // Candidate conflicts on both a and b; either name is acceptable.
        let bad = vec![Value::Int64(2), Value::Int64(10), Value::Int64(20)];
        let err = c.check(&bad, existing).unwrap_err();
        assert!(err.constraint == "uniq_a" || err.constraint == "uniq_b");
    }
}
