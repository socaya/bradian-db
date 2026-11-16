//! ADR 0098 — typed authorization capabilities foundation.
//!
//! A tiny capability model that higher-layer SQL engines can
//! consult before dispatching a statement. **Not** enterprise
//! identity, **not** TLS/mTLS, **not** key custody — those
//! are INV-003 production obligations that still require §13
//! owner decisions on PKI/identity.
//!
//! The types here are the foundation an operator-facing
//! authz-aware `execute_as()` method can call against.

use serde::{Deserialize, Serialize};

/// Granularity of permission required to run a SQL statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Capability {
    /// `SELECT` — read-only access.
    Read,
    /// `INSERT`, `UPDATE`, `DELETE` — row-mutating statements.
    Write,
    /// `CREATE`, `DROP`, `ALTER` — schema-mutating statements.
    Ddl,
    /// Reserved for future administrative operations
    /// (rotation, retention floor advance, key custody). Not
    /// yet reachable through SQL.
    Admin,
}

impl Capability {
    /// Minimal capability required by the first keyword of a
    /// SQL statement. Returns `None` for unknown or empty
    /// statements so the caller can decide whether to reject
    /// (strict) or let the parser produce the real error
    /// (permissive).
    pub fn required_for_sql(sql: &str) -> Option<Self> {
        let first = sql.trim().split_ascii_whitespace().next()?;
        let keyword: String = first.chars().map(|c| c.to_ascii_lowercase()).collect();
        match keyword.as_str() {
            "select" | "explain" | "show" | "describe" => Some(Capability::Read),
            "insert" | "update" | "delete" | "upsert" | "merge" => Some(Capability::Write),
            "create" | "drop" | "alter" | "truncate" | "rename" => Some(Capability::Ddl),
            "begin" | "commit" | "rollback" | "savepoint" | "release" => Some(Capability::Write),
            _ => None,
        }
    }
}

/// Bundle of a principal and the capabilities it may exercise.
/// Opaque principal id so this crate does not need to pull in
/// bradian-security's richer `Principal` type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizationContext {
    principal_id: String,
    capabilities: Vec<Capability>,
}

impl AuthorizationContext {
    /// Construct with explicit capabilities.
    pub fn new(principal_id: impl Into<String>, capabilities: Vec<Capability>) -> Self {
        Self { principal_id: principal_id.into(), capabilities }
    }

    /// Administrator bootstrap: holds all capabilities. Reserved
    /// for in-process use; production deployments must derive
    /// the principal from a verified session (INV-003 open).
    pub fn administrator(principal_id: impl Into<String>) -> Self {
        Self::new(
            principal_id,
            vec![Capability::Read, Capability::Write, Capability::Ddl, Capability::Admin],
        )
    }

    /// Read-only context — used by analyst roles that may not
    /// write or DDL.
    pub fn read_only(principal_id: impl Into<String>) -> Self {
        Self::new(principal_id, vec![Capability::Read])
    }

    pub fn principal_id(&self) -> &str { &self.principal_id }
    pub fn capabilities(&self) -> &[Capability] { &self.capabilities }

    pub fn allows(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }

    /// True iff the context allows the first keyword of the
    /// given SQL. Unknown keywords return `true` (the SQL
    /// parser will produce the real error) so this check is
    /// strictly a fast-fail layer, not the parser's substitute.
    pub fn allows_sql(&self, sql: &str) -> bool {
        match Capability::required_for_sql(sql) {
            Some(c) => self.allows(c),
            None => true,
        }
    }
}

/// Error returned by authz-aware execute wrappers when the
/// context lacks the capability for the attempted statement.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("authorization denied: principal {principal_id:?} lacks capability {required:?}")]
pub struct AuthorizationDenied {
    pub principal_id: String,
    pub required: Capability,
}

impl AuthorizationContext {
    /// Fast-fail check used by `SqlEngine::execute_as`. Returns
    /// `Ok(())` when the context allows the statement's
    /// required capability or when the statement has no
    /// detectable required capability.
    pub fn check_sql(&self, sql: &str) -> Result<(), AuthorizationDenied> {
        match Capability::required_for_sql(sql) {
            Some(required) if !self.allows(required) => Err(AuthorizationDenied {
                principal_id: self.principal_id.clone(),
                required,
            }),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_for_sql_classifies_keywords() {
        assert_eq!(Capability::required_for_sql("SELECT 1"), Some(Capability::Read));
        assert_eq!(Capability::required_for_sql("  insert into t "), Some(Capability::Write));
        assert_eq!(Capability::required_for_sql("CREATE DATABASE x"), Some(Capability::Ddl));
        assert_eq!(Capability::required_for_sql("BEGIN"), Some(Capability::Write));
        assert_eq!(Capability::required_for_sql(""), None);
        assert_eq!(Capability::required_for_sql("    "), None);
        assert_eq!(Capability::required_for_sql("unknown-word-x"), None);
    }

    #[test]
    fn administrator_allows_everything() {
        let a = AuthorizationContext::administrator("root");
        assert!(a.allows(Capability::Read));
        assert!(a.allows(Capability::Write));
        assert!(a.allows(Capability::Ddl));
        assert!(a.allows(Capability::Admin));
        assert!(a.check_sql("SELECT 1").is_ok());
        assert!(a.check_sql("CREATE DATABASE x").is_ok());
    }

    #[test]
    fn read_only_rejects_writes_and_ddl() {
        let r = AuthorizationContext::read_only("analyst");
        assert!(r.check_sql("SELECT 1").is_ok());
        let err = r.check_sql("INSERT INTO t VALUES (1)").unwrap_err();
        assert_eq!(err.required, Capability::Write);
        let err = r.check_sql("CREATE DATABASE x").unwrap_err();
        assert_eq!(err.required, Capability::Ddl);
    }

    #[test]
    fn unknown_keyword_passes_authz_so_parser_produces_the_real_error() {
        let r = AuthorizationContext::read_only("analyst");
        // "FOOBAR 1" is not recognized as a known keyword;
        // authz should let the parser handle it.
        assert!(r.check_sql("FOOBAR 1").is_ok());
    }
}
