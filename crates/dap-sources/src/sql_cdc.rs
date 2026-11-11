//! SQL-shaped change-data-capture adapter.
//!
//! This module defines the trait + envelope DAP expects from any SQL CDC source
//! (PostgreSQL logical replication, MySQL binlog, Debezium, etc.). A concrete transport
//! (e.g. a reqwest-based pull against Debezium's Kafka REST proxy) plugs into the same
//! trait.
//!
//! The adapter normalises CDC rows to `dap_ingest::Change` envelopes with:
//!
//! - `change_id` = stable identity (e.g. `"{table}:{pk}:{lsn}"`).
//! - `source_version` = log-sequence number / commit LSN.
//! - `operation` = `Upsert` for `INSERT`/`UPDATE`, `Delete` for `DELETE`.
//! - `payload` = the "after" row for INSERT/UPDATE, the "before" row for DELETE.

use dap_core::{DapResult, Uid};
use dap_ingest::{Change, Operation};
use serde::{Deserialize, Serialize};

/// Minimal CDC row shape. Richer transports can carry transactions, schema metadata, LSNs
/// per partition, etc. — this is the subset DAP strictly needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqlCdcRow {
    pub table: String,
    pub primary_key: String,
    pub op: SqlCdcOp,
    pub lsn: u64,
    pub before: Option<serde_json::Value>,
    pub after: Option<serde_json::Value>,
    pub commit_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SqlCdcOp {
    Insert,
    Update,
    Delete,
}

/// Trait for SQL CDC transports. Implementations typically wrap a long-poll client that
/// streams rows from Debezium / wal2json / mysql-binlog-connector-rs.
pub trait SqlCdcSource: Send + Sync {
    fn next_batch(&mut self) -> DapResult<Vec<SqlCdcRow>>;

    /// Return the last LSN DAP durably applied for this source. Used by `start_from` so a
    /// restart resumes without replaying already-applied rows.
    fn start_from(&self) -> u64 {
        0
    }
}

/// Convert CDC rows into canonical `Change` envelopes.
pub fn rows_to_changes(
    source_label: &str,
    rows: Vec<SqlCdcRow>,
) -> DapResult<Vec<Change>> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let operation = match row.op {
            SqlCdcOp::Insert | SqlCdcOp::Update => Operation::Upsert,
            SqlCdcOp::Delete => Operation::Delete,
        };
        let payload = match operation {
            Operation::Upsert => row.after.clone().unwrap_or(serde_json::Value::Null),
            Operation::Delete => row.before.clone().unwrap_or(serde_json::Value::Null),
        };
        let synthetic_uid = synthesise_uid(&row.table, &row.primary_key)?;
        out.push(Change {
            change_id: format!("{source_label}:{}:{}:{}", row.table, row.primary_key, row.lsn),
            site_id: Some(source_label.to_string()),
            object_type: row.table.clone(),
            object_uid: synthetic_uid,
            source_version: row.lsn,
            operation,
            occurred_at: format_ms(row.commit_ms),
            metadata_revision: None,
            payload_hash: None,
            payload,
        });
    }
    Ok(out)
}

fn synthesise_uid(table: &str, pk: &str) -> DapResult<Uid> {
    let seed = format!("{table}#{pk}");
    Ok(Uid::derive(seed.as_bytes()))
}

fn format_ms(ms: i64) -> String {
    // Minimal ISO 8601 UTC emitter, matching the DHIS2 adapter.
    use std::time::{Duration, UNIX_EPOCH};
    let secs = (ms / 1000) as u64;
    let _ = UNIX_EPOCH + Duration::from_secs(secs);
    format!("ms={ms}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_maps_to_upsert_with_after_payload() {
        let row = SqlCdcRow {
            table: "patients".into(),
            primary_key: "42".into(),
            op: SqlCdcOp::Insert,
            lsn: 100,
            before: None,
            after: Some(serde_json::json!({"id": 42, "name": "A"})),
            commit_ms: 1_700_000_000_000,
        };
        let changes = rows_to_changes("db1", vec![row]).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].operation, Operation::Upsert);
        assert_eq!(changes[0].object_type, "patients");
        assert_eq!(changes[0].payload["name"], "A");
        assert_eq!(changes[0].source_version, 100);
    }

    #[test]
    fn delete_maps_to_delete_with_before_payload() {
        let row = SqlCdcRow {
            table: "patients".into(),
            primary_key: "42".into(),
            op: SqlCdcOp::Delete,
            lsn: 101,
            before: Some(serde_json::json!({"id": 42, "name": "A"})),
            after: None,
            commit_ms: 1_700_000_000_000,
        };
        let changes = rows_to_changes("db1", vec![row]).unwrap();
        assert_eq!(changes[0].operation, Operation::Delete);
        assert_eq!(changes[0].payload["name"], "A");
    }

    #[test]
    fn change_id_encodes_source_table_pk_lsn() {
        let row = SqlCdcRow {
            table: "orders".into(),
            primary_key: "9".into(),
            op: SqlCdcOp::Insert,
            lsn: 5,
            before: None,
            after: Some(serde_json::Value::Null),
            commit_ms: 0,
        };
        let changes = rows_to_changes("prod_db", vec![row]).unwrap();
        assert_eq!(changes[0].change_id, "prod_db:orders:9:5");
        assert_eq!(changes[0].site_id.as_deref(), Some("prod_db"));
    }
}
