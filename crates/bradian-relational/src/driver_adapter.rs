//! ADR 0158 — SqlEngine driver adapter.
//!
//! BRD-WIRE-001 "portable client driver API" moves from
//! FOUNDATIONS to PARTIAL: ADR 0129 shipped the `Driver`
//! trait; this module wires the base `SqlEngine` behind it.
//! Clients that depend on `dyn Driver` now get real
//! execution instead of the `NoopDriver` stub.
//!
//! Honest scope:
//! - `SqlEngineAdapter { engine: Arc<SqlEngine> }` holds
//!   an `Arc` so the adapter is cheaply cloneable and the
//!   engine's lifetime spans every outstanding driver
//!   handle.
//! - `impl Driver` routes each `execute(sql)` to
//!   `engine.execute(sql)`, translating `SqlResultSet` to
//!   `DriverResponse`.
//! - Error translation: `SqlError` to
//!   `DriverError::Sql(message)` for unknown engine
//!   errors; `SqlError::Budget` /
//!   `SqlError::Resource` to `DriverError::Resource`.
//!
//! Out of scope (follow-ups):
//! - Session-based engines (`HistorySqlEngine`,
//!   `DomainSqlEngine`, `MvccSqlEngine`) — each needs its
//!   own adapter that holds a persistent session handle.
//! - Prepared-statement execution via ADR 0139 envelope.
//! - Network transport (wraps this adapter + ADR 0137
//!   frames).
//! - Streaming row delivery (ADR 0140 chunk assembler).
//! - TLS/mTLS.

use std::sync::Arc;

use crate::driver::{Driver, DriverError, DriverResponse, DriverResult, DriverRow};
use crate::sql::{SqlEngine, SqlError, SqlResultSet};

/// ADR 0158 — Driver adapter wrapping a shared
/// `SqlEngine`. Cheap to clone; all clones share the
/// same engine via `Arc`.
#[derive(Clone)]
pub struct SqlEngineAdapter {
    engine: Arc<SqlEngine>,
}

impl std::fmt::Debug for SqlEngineAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlEngineAdapter").finish_non_exhaustive()
    }
}

impl SqlEngineAdapter {
    pub fn new(engine: Arc<SqlEngine>) -> Self {
        Self { engine }
    }
}

impl Driver for SqlEngineAdapter {
    fn execute(&self, sql: &str) -> DriverResult<DriverResponse> {
        match self.engine.execute(sql) {
            Ok(rs) => Ok(translate_result(rs)),
            Err(err) => Err(translate_error(err)),
        }
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
    use tempfile::tempdir;

    fn open_adapter() -> (tempfile::TempDir, SqlEngineAdapter) {
        let dir = tempdir().unwrap();
        let engine = Arc::new(SqlEngine::open(dir.path()).unwrap());
        (dir, SqlEngineAdapter::new(engine))
    }

    #[test]
    fn adapter_executes_ddl_and_dml() {
        let (_d, adapter) = open_adapter();
        // CREATE path returns affected=0 → DriverResponse::Empty.
        let rs = adapter.execute("CREATE DATABASE app").unwrap();
        assert_eq!(rs, DriverResponse::Empty);
        adapter.execute("CREATE SCHEMA app.public").unwrap();
        adapter.execute("CREATE TABLE app.public.items (
            id BIGINT NOT NULL, name TEXT NOT NULL,
            PRIMARY KEY (id))").unwrap();
        // INSERT returns affected=1 → DriverResponse::Affected.
        let rs = adapter.execute(
            "INSERT INTO app.public.items (id, name) VALUES (1, 'hi')").unwrap();
        assert_eq!(rs, DriverResponse::Affected { rows: 1 });
    }

    #[test]
    fn adapter_translates_select_to_rows() {
        let (_d, adapter) = open_adapter();
        adapter.execute("CREATE DATABASE a").unwrap();
        adapter.execute("CREATE SCHEMA a.pub").unwrap();
        adapter.execute("CREATE TABLE a.pub.t (
            id BIGINT NOT NULL, PRIMARY KEY (id))").unwrap();
        adapter.execute("INSERT INTO a.pub.t (id) VALUES (42)").unwrap();
        let rs = adapter.execute("SELECT id FROM a.pub.t").unwrap();
        match rs {
            DriverResponse::Rows(rows) => {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].columns, vec!["id".to_string()]);
            }
            other => panic!("expected Rows, got {other:?}"),
        }
    }

    #[test]
    fn adapter_translates_syntax_error() {
        let (_d, adapter) = open_adapter();
        let err = adapter.execute("NOT SQL AT ALL").unwrap_err();
        assert!(matches!(err, DriverError::Sql(_)));
    }

    #[test]
    fn adapter_is_object_safe_behind_dyn_driver() {
        let (_d, adapter) = open_adapter();
        let as_dyn: Box<dyn Driver> = Box::new(adapter);
        let rs = as_dyn.execute("CREATE DATABASE x").unwrap();
        assert_eq!(rs, DriverResponse::Empty);
    }

    #[test]
    fn clone_shares_engine_state() {
        let (_d, adapter_a) = open_adapter();
        let adapter_b = adapter_a.clone();
        adapter_a.execute("CREATE DATABASE shared").unwrap();
        // Clone B sees the state A created.
        let err = adapter_b.execute("CREATE DATABASE shared").unwrap_err();
        // Duplicate database name errors.
        assert!(matches!(err, DriverError::Sql(_)));
    }
}
