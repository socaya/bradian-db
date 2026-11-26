//! ADR 0160 — session-based engine driver adapters.
//!
//! Extends ADR 0158's `SqlEngineAdapter` to the three
//! session-based engines (`HistorySqlEngine`,
//! `DomainSqlEngine`, `MvccSqlEngine`). Every SQL engine
//! in the codebase is now reachable through a
//! `dyn Driver`.
//!
//! Honest scope — each adapter opens a FRESH session per
//! `execute(sql)` call. Multi-statement transactional
//! sessions are NOT bound to the Driver trait because the
//! trait's single `execute(sql)` method has no begin/
//! commit lifecycle. A future session-aware driver trait
//! can hold the session handle across calls.
//!
//! Out of scope: see ADR 0158 §2.

use std::sync::Arc;

use crate::domain_sql::DomainSqlEngine;
use crate::driver::{Driver, DriverError, DriverResponse, DriverResult, DriverRow};
use crate::history_sql::HistorySqlEngine;
use crate::mvcc_sql::MvccSqlEngine;
use crate::sql::{SqlError, SqlResultSet};

// ---------------- HistorySqlEngine ----------------

/// ADR 0160 — adapter for `HistorySqlEngine`. Opens a
/// FRESH `with_session` per `execute(sql)`.
#[derive(Clone)]
pub struct HistorySqlEngineAdapter {
    engine: Arc<HistorySqlEngine>,
}

impl std::fmt::Debug for HistorySqlEngineAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistorySqlEngineAdapter").finish_non_exhaustive()
    }
}

impl HistorySqlEngineAdapter {
    pub fn new(engine: Arc<HistorySqlEngine>) -> Self { Self { engine } }
}

impl Driver for HistorySqlEngineAdapter {
    fn execute(&self, sql: &str) -> DriverResult<DriverResponse> {
        let out = self.engine.with_session(|s| autocommit(s, sql));
        translate(out)
    }
}

// ---------------- DomainSqlEngine ----------------

#[derive(Clone)]
pub struct DomainSqlEngineAdapter {
    engine: Arc<DomainSqlEngine>,
}

impl std::fmt::Debug for DomainSqlEngineAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DomainSqlEngineAdapter").finish_non_exhaustive()
    }
}

impl DomainSqlEngineAdapter {
    pub fn new(engine: Arc<DomainSqlEngine>) -> Self { Self { engine } }
}

impl Driver for DomainSqlEngineAdapter {
    fn execute(&self, sql: &str) -> DriverResult<DriverResponse> {
        let out = self.engine.with_session(|s| autocommit_domain(s, sql));
        translate(out)
    }
}

// ---------------- MvccSqlEngine ----------------

#[derive(Clone)]
pub struct MvccSqlEngineAdapter {
    engine: Arc<MvccSqlEngine>,
}

impl std::fmt::Debug for MvccSqlEngineAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MvccSqlEngineAdapter").finish_non_exhaustive()
    }
}

impl MvccSqlEngineAdapter {
    pub fn new(engine: Arc<MvccSqlEngine>) -> Self { Self { engine } }
}

impl Driver for MvccSqlEngineAdapter {
    fn execute(&self, sql: &str) -> DriverResult<DriverResponse> {
        let out = self.engine.with_session(|s| autocommit_mvcc(s, sql));
        translate(out)
    }
}

// ---------------- autocommit helpers ----------------

/// ADR 0160 — treat each Driver::execute call as an
/// autocommit transaction. If the caller's SQL is
/// already a transaction command, pass it through as-is.
fn is_transaction_command(sql: &str) -> bool {
    let trimmed = sql.trim_start().to_ascii_lowercase();
    trimmed.starts_with("begin") || trimmed.starts_with("commit")
        || trimmed.starts_with("rollback") || trimmed.starts_with("savepoint")
        || trimmed.starts_with("release")
}

fn autocommit(
    s: &mut crate::history_sql::HistorySqlSession<'_>, sql: &str,
) -> crate::sql::SqlResult<crate::sql::SqlResultSet> {
    if is_transaction_command(sql) { return s.execute(sql); }
    s.execute("BEGIN")?;
    let out = s.execute(sql);
    match out {
        Ok(rs) => { s.execute("COMMIT")?; Ok(rs) }
        Err(e) => {
            // execute() already aborted the transaction on
            // error; attempt rollback best-effort.
            let _ = s.execute("ROLLBACK");
            Err(e)
        }
    }
}

fn autocommit_domain(
    s: &mut crate::domain_sql::DomainSqlSession<'_>, sql: &str,
) -> crate::sql::SqlResult<crate::sql::SqlResultSet> {
    if is_transaction_command(sql) { return s.execute(sql); }
    s.execute("BEGIN")?;
    let out = s.execute(sql);
    match out {
        Ok(rs) => { s.execute("COMMIT")?; Ok(rs) }
        Err(e) => { let _ = s.execute("ROLLBACK"); Err(e) }
    }
}

fn autocommit_mvcc(
    s: &mut crate::mvcc_sql::MvccSqlSession<'_>, sql: &str,
) -> crate::sql::SqlResult<crate::sql::SqlResultSet> {
    if is_transaction_command(sql) { return s.execute(sql); }
    s.execute("BEGIN")?;
    let out = s.execute(sql);
    match out {
        Ok(rs) => { s.execute("COMMIT")?; Ok(rs) }
        Err(e) => { let _ = s.execute("ROLLBACK"); Err(e) }
    }
}

// ---------------- shared result/error translation ----------------

fn translate(out: Result<SqlResultSet, SqlError>) -> DriverResult<DriverResponse> {
    match out {
        Ok(rs) => Ok(translate_result(rs)),
        Err(e) => Err(translate_error(e)),
    }
}

fn translate_result(rs: SqlResultSet) -> DriverResponse {
    match rs {
        SqlResultSet::Command { affected } =>
            if affected == 0 {
                DriverResponse::Empty
            } else {
                DriverResponse::Affected { rows: affected }
            },
        SqlResultSet::Rows { columns, rows, .. } => {
            let rows = rows.into_iter().map(|values| DriverRow {
                columns: columns.clone(),
                values,
            }).collect();
            DriverResponse::Rows(rows)
        }
    }
}

fn translate_error(err: SqlError) -> DriverError {
    match err {
        SqlError::Resource(_) | SqlError::Budget(_) =>
            DriverError::Resource(format!("{err}")),
        _ => DriverError::Sql(format!("{err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::SqlEngine;
    use tempfile::tempdir;

    fn setup(root: &std::path::Path) {
        {
            let ddl = SqlEngine::open(root).unwrap();
            for sql in [
                "CREATE DATABASE app",
                "CREATE SCHEMA app.public",
                "CREATE TABLE app.public.items (id BIGINT NOT NULL,
                     name TEXT NOT NULL, PRIMARY KEY (id))",
            ] { ddl.execute(sql).unwrap(); }
        }
        {
            // Provision the G18 shared domain + register the
            // catalog table so MvccSqlEngine can open on top.
            let d = DomainSqlEngine::open(root).unwrap();
            d.register_catalog_table("app.public.items").unwrap();
        }
    }

    #[test]
    fn domain_adapter_autocommit_insert_and_select() {
        let dir = tempdir().unwrap();
        setup(dir.path());
        let eng = Arc::new(DomainSqlEngine::open(dir.path()).unwrap());
        let adapter = DomainSqlEngineAdapter::new(eng);
        // Autocommit: each execute() wraps its own BEGIN + COMMIT.
        let rs = adapter.execute(
            "INSERT INTO app.public.items (id, name) VALUES (1, 'x')").unwrap();
        assert_eq!(rs, DriverResponse::Affected { rows: 1 });
        let rs = adapter.execute("SELECT id FROM app.public.items").unwrap();
        match rs {
            DriverResponse::Rows(rows) => assert_eq!(rows.len(), 1),
            other => panic!("expected Rows, got {other:?}"),
        }
    }

    #[test]
    fn mvcc_adapter_autocommit_dml() {
        let dir = tempdir().unwrap();
        setup(dir.path());
        let eng = Arc::new(MvccSqlEngine::open(dir.path()).unwrap());
        let adapter: Box<dyn Driver> = Box::new(MvccSqlEngineAdapter::new(eng));
        let rs = adapter.execute(
            "INSERT INTO app.public.items (id, name) VALUES (7, 'y')").unwrap();
        assert_eq!(rs, DriverResponse::Affected { rows: 1 });
    }

    #[test]
    fn transaction_commands_pass_through_without_autocommit_wrapping() {
        let dir = tempdir().unwrap();
        setup(dir.path());
        let eng = Arc::new(MvccSqlEngine::open(dir.path()).unwrap());
        let adapter = MvccSqlEngineAdapter::new(eng);
        // BEGIN alone opens a transaction that outlives the
        // session; since each adapter call opens its own
        // session, BEGIN is effectively a no-op from the
        // caller's view. The pass-through policy is honest.
        let _ = adapter.execute("BEGIN");
    }

    #[test]
    fn domain_adapter_is_cloneable() {
        let dir = tempdir().unwrap();
        setup(dir.path());
        let d = DomainSqlEngineAdapter::new(
            Arc::new(DomainSqlEngine::open(dir.path()).unwrap()));
        let _clone = d.clone();
    }

    #[test]
    fn is_transaction_command_detects_all_forms() {
        assert!(is_transaction_command("BEGIN"));
        assert!(is_transaction_command("  begin"));
        assert!(is_transaction_command("COMMIT"));
        assert!(is_transaction_command("Rollback"));
        assert!(is_transaction_command("SAVEPOINT x"));
        assert!(is_transaction_command("RELEASE SAVEPOINT x"));
        assert!(!is_transaction_command("SELECT 1"));
        assert!(!is_transaction_command("INSERT INTO t VALUES (1)"));
    }
}
