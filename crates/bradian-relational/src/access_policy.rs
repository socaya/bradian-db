//! ADR 0123 — row / column / tenant access policy foundation.
//!
//! BRD-SEC-002 "RBAC/ABAC, row/column/tenant policies end-
//! to-end" is PARTIAL. This module ships the EVALUATION
//! PRIMITIVES a future authoritative-enforcement path can
//! call. It does NOT wire itself into any SQL engine today;
//! callers evaluate a policy explicitly against a candidate
//! row + principal.
//!
//! Honest scope:
//! - Row-level security (RLS): a per-table predicate the
//!   evaluator checks against each candidate row.
//! - Column-level security (CLS): an allow-list of columns
//!   a principal may READ. Writes bypass this check (CLS is
//!   a projection concept, not a mutation concept).
//! - Tenant isolation: a per-row `tenant_id` column + a
//!   principal-held tenant scope; rows outside scope are
//!   filtered.
//!
//! Non-goals:
//! - Policy storage in the catalog (where do policies
//!   live? follow-up ADR).
//! - Automatic application by SQL engines.
//! - Hierarchical tenant trees / parent-child scoping.

use std::collections::BTreeSet;

use bradian_core::AuthorizationContext;
use thiserror::Error;

use crate::typed_table::Value;

/// A simple row predicate the evaluator can check against a
/// candidate row. Minimal: equals / not-equals on a named
/// column vs a literal. Richer predicates belong in a
/// future ADR that defines a policy DSL.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RowPredicate {
    ColumnEquals { column: String, value: Value },
    ColumnNotEquals { column: String, value: Value },
    /// Accepts any row (useful as default-allow).
    AlwaysAllow,
    /// Rejects every row (useful as default-deny).
    AlwaysDeny,
}

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("column {column:?} not found in table shape")]
    ColumnNotFound { column: String },
    #[error("tenant isolation column {column:?} must be non-null")]
    TenantColumnNull { column: String },
    #[error("tenant isolation column {column:?} must be a string")]
    TenantColumnNotString { column: String },
    /// ADR 0155 — caller explicitly asked for a column the
    /// policy's column-level allow-list does not permit.
    #[error("column {column:?} denied by column-level policy")]
    ColumnDenied { column: String },
}

pub type Result<T> = std::result::Result<T, PolicyError>;

/// Row-level security policy for a single table. Fields
/// address columns by name so the policy doesn't bind to a
/// column-index that could shift after ALTER TABLE.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RowLevelPolicy {
    pub predicate: RowPredicate,
}

impl RowLevelPolicy {
    pub fn allow_all() -> Self {
        Self { predicate: RowPredicate::AlwaysAllow }
    }

    /// Evaluate against a candidate row given the column
    /// name → index mapping.
    pub fn allows(&self, row: &[Value], columns: &[String]) -> Result<bool> {
        match &self.predicate {
            RowPredicate::AlwaysAllow => Ok(true),
            RowPredicate::AlwaysDeny => Ok(false),
            RowPredicate::ColumnEquals { column, value } => {
                let idx = columns.iter().position(|c| c == column)
                    .ok_or_else(|| PolicyError::ColumnNotFound { column: column.clone() })?;
                Ok(&row[idx] == value)
            }
            RowPredicate::ColumnNotEquals { column, value } => {
                let idx = columns.iter().position(|c| c == column)
                    .ok_or_else(|| PolicyError::ColumnNotFound { column: column.clone() })?;
                Ok(&row[idx] != value)
            }
        }
    }
}

/// Column-level security: the allow-list of columns a
/// principal may READ. Writes are not filtered — CLS is a
/// projection concept.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ColumnLevelPolicy {
    pub readable: BTreeSet<String>,
}

impl ColumnLevelPolicy {
    pub fn allow_all(table_columns: &[String]) -> Self {
        Self { readable: table_columns.iter().cloned().collect() }
    }

    pub fn deny_all() -> Self { Self { readable: BTreeSet::new() } }

    pub fn allows_read(&self, column: &str) -> bool {
        self.readable.contains(column)
    }

    /// ADR 0155 — a policy is "configured" for engine
    /// integration iff it carries an explicit allow-list.
    /// An empty `readable` set means the caller passed a
    /// placeholder (CLS not opted in); the engine skips
    /// CLS filtering in that case.
    pub fn is_configured(&self) -> bool { !self.readable.is_empty() }

    /// Project a row to only the columns this policy allows.
    /// Returns the (allowed_column_names, filtered_values)
    /// pair preserving input order of the filtered columns.
    pub fn project(&self, row: &[Value], columns: &[String]) -> (Vec<String>, Vec<Value>) {
        let mut names = Vec::new();
        let mut values = Vec::new();
        for (i, col) in columns.iter().enumerate() {
            if self.readable.contains(col) {
                names.push(col.clone());
                values.push(row[i].clone());
            }
        }
        (names, values)
    }
}

/// Tenant isolation: the principal's authorized tenant
/// scope + the per-table tenant-column name. Rows whose
/// tenant column isn't in the principal's scope are
/// filtered.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TenantIsolation {
    pub tenant_column: String,
    pub allowed_tenants: BTreeSet<String>,
}

impl TenantIsolation {
    pub fn single_tenant(tenant_column: impl Into<String>, tenant: impl Into<String>) -> Self {
        let mut s = BTreeSet::new();
        s.insert(tenant.into());
        Self { tenant_column: tenant_column.into(), allowed_tenants: s }
    }

    pub fn allows(&self, row: &[Value], columns: &[String]) -> Result<bool> {
        let idx = columns.iter().position(|c| c == &self.tenant_column)
            .ok_or_else(|| PolicyError::ColumnNotFound {
                column: self.tenant_column.clone(),
            })?;
        match &row[idx] {
            Value::Null => Err(PolicyError::TenantColumnNull {
                column: self.tenant_column.clone(),
            }),
            Value::String(s) => Ok(self.allowed_tenants.contains(s)),
            _ => Err(PolicyError::TenantColumnNotString {
                column: self.tenant_column.clone(),
            }),
        }
    }
}

/// Composite policy a caller can evaluate against a
/// candidate row. Passes only if ALL three sub-policies
/// pass. The `_authz` parameter is a hook for future
/// capability-aware refinements (ADR 0098).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TableAccessPolicy {
    pub row_level: RowLevelPolicy,
    pub column_level: ColumnLevelPolicy,
    pub tenant: Option<TenantIsolation>,
}

impl TableAccessPolicy {
    pub fn allows_row(&self, row: &[Value], columns: &[String],
                       _authz: &AuthorizationContext) -> Result<bool> {
        if !self.row_level.allows(row, columns)? { return Ok(false); }
        if let Some(t) = &self.tenant {
            if !t.allows(row, columns)? { return Ok(false); }
        }
        Ok(true)
    }

    pub fn project_row(&self, row: &[Value], columns: &[String])
        -> (Vec<String>, Vec<Value>) {
        self.column_level.project(row, columns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bradian_core::AuthorizationContext;

    fn cols() -> Vec<String> {
        vec!["id".into(), "tenant".into(), "name".into(), "salary".into()]
    }

    fn row_alice() -> Vec<Value> {
        vec![Value::Int64(1), Value::String("acme".into()),
             Value::String("alice".into()), Value::Int64(100_000)]
    }

    fn row_bob() -> Vec<Value> {
        vec![Value::Int64(2), Value::String("widgets".into()),
             Value::String("bob".into()), Value::Int64(80_000)]
    }

    #[test]
    fn row_level_allow_all_passes_everything() {
        let p = RowLevelPolicy::allow_all();
        assert!(p.allows(&row_alice(), &cols()).unwrap());
    }

    #[test]
    fn row_level_column_equals_filters() {
        let p = RowLevelPolicy {
            predicate: RowPredicate::ColumnEquals {
                column: "name".into(),
                value: Value::String("alice".into()),
            }
        };
        assert!(p.allows(&row_alice(), &cols()).unwrap());
        assert!(!p.allows(&row_bob(), &cols()).unwrap());
    }

    #[test]
    fn row_level_unknown_column_errors() {
        let p = RowLevelPolicy {
            predicate: RowPredicate::ColumnEquals {
                column: "nope".into(),
                value: Value::Int64(0),
            }
        };
        assert!(p.allows(&row_alice(), &cols()).is_err());
    }

    #[test]
    fn column_level_projection_filters_out_denied_columns() {
        let mut policy = ColumnLevelPolicy::deny_all();
        policy.readable.insert("id".into());
        policy.readable.insert("name".into());
        let (names, values) = policy.project(&row_alice(), &cols());
        assert_eq!(names, vec!["id", "name"]);
        assert_eq!(values.len(), 2);
    }

    #[test]
    fn column_level_allow_all_returns_full_row() {
        let policy = ColumnLevelPolicy::allow_all(&cols());
        let (names, values) = policy.project(&row_alice(), &cols());
        assert_eq!(names.len(), cols().len());
        assert_eq!(values.len(), row_alice().len());
    }

    #[test]
    fn tenant_isolation_allows_in_scope_rejects_out() {
        let t = TenantIsolation::single_tenant("tenant", "acme");
        assert!(t.allows(&row_alice(), &cols()).unwrap());
        assert!(!t.allows(&row_bob(), &cols()).unwrap());
    }

    #[test]
    fn tenant_isolation_null_column_errors() {
        let t = TenantIsolation::single_tenant("tenant", "acme");
        let row = vec![Value::Int64(1), Value::Null,
                       Value::String("x".into()), Value::Int64(0)];
        let err = t.allows(&row, &cols()).unwrap_err();
        assert!(matches!(err, PolicyError::TenantColumnNull { .. }));
    }

    #[test]
    fn composite_policy_composes_row_level_and_tenant() {
        let p = TableAccessPolicy {
            row_level: RowLevelPolicy {
                predicate: RowPredicate::ColumnNotEquals {
                    column: "name".into(),
                    value: Value::String("eve".into()),
                },
            },
            column_level: ColumnLevelPolicy::allow_all(&cols()),
            tenant: Some(TenantIsolation::single_tenant("tenant", "acme")),
        };
        let authz = AuthorizationContext::administrator("root");
        assert!(p.allows_row(&row_alice(), &cols(), &authz).unwrap());
        // Bob fails tenant check.
        assert!(!p.allows_row(&row_bob(), &cols(), &authz).unwrap());
    }

    #[test]
    fn composite_policy_denies_when_row_level_is_always_deny() {
        let p = TableAccessPolicy {
            row_level: RowLevelPolicy { predicate: RowPredicate::AlwaysDeny },
            column_level: ColumnLevelPolicy::allow_all(&cols()),
            tenant: None,
        };
        let authz = AuthorizationContext::administrator("root");
        assert!(!p.allows_row(&row_alice(), &cols(), &authz).unwrap());
    }
}
