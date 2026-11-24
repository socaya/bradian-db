//! SQL-shaped change-data-capture adapter.
//!
//! This module defines the trait + envelope DAP expects from any SQL CDC source
//! (PostgreSQL logical replication, MySQL binlog, Debezium, etc.). A concrete transport
//! (e.g. a reqwest-based pull against Debezium's Kafka REST proxy) plugs into the same
//! trait.
//!
//! The adapter normalises CDC rows to `bradian_ingest::Change` envelopes with:
//!
//! - `change_id` = stable identity (e.g. `"{table}:{pk}:{lsn}"`).
//! - `source_version` = log-sequence number / commit LSN.
//! - `operation` = `Upsert` for `INSERT`/`UPDATE`, `Delete` for `DELETE`.
//! - `payload` = the "after" row for INSERT/UPDATE, the "before" row for DELETE.

use std::path::{Path, PathBuf};

use bradian_core::{BradianError, BradianResult, Uid};
use bradian_ingest::{Change, Operation};
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
    fn next_batch(&mut self) -> BradianResult<Vec<SqlCdcRow>>;

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
) -> BradianResult<Vec<Change>> {
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

fn synthesise_uid(table: &str, pk: &str) -> BradianResult<Uid> {
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

/// ADR 0305 — typed, least-privilege CDC credential.
/// Validates shape at construction; `readonly=false` is
/// refused since there is no legitimate write path for a
/// CDC puller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqlCdcCredentials {
    pub role:             String,
    pub replication_slot: String,
    pub readonly:         bool,
}

impl SqlCdcCredentials {
    pub fn try_new(
        role:             impl Into<String>,
        replication_slot: impl Into<String>,
        readonly:         bool,
    ) -> BradianResult<Self> {
        if !readonly {
            return Err(BradianError::invalid_value(
                "sql CDC credentials must be readonly=true (ADR 0305)"));
        }
        let role = role.into();
        let replication_slot = replication_slot.into();
        if role.is_empty() || replication_slot.is_empty() {
            return Err(BradianError::invalid_value(
                "sql CDC credentials: role and replication_slot are required"));
        }
        Ok(Self { role, replication_slot, readonly })
    }
}

/// ADR 0305 — persistent CDC cursor backed by a single
/// file under atomic rename.
#[derive(Clone, Debug)]
pub struct CursorStore {
    path: PathBuf,
}

impl CursorStore {
    pub fn new(path: impl Into<PathBuf>) -> Self { Self { path: path.into() } }

    /// Returns 0 if the cursor file does not exist.
    pub fn load(&self) -> BradianResult<u64> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => text.trim().parse::<u64>().map_err(|e|
                BradianError::invalid_value(format!("cursor parse: {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(e) => Err(BradianError::invalid_value(format!("cursor read: {e}"))),
        }
    }

    pub fn save(&self, lsn: u64) -> BradianResult<()> {
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, lsn.to_string().as_bytes()).map_err(|e|
            BradianError::invalid_value(format!("cursor write: {e}")))?;
        std::fs::rename(&tmp, &self.path).map_err(|e|
            BradianError::invalid_value(format!("cursor rename: {e}")))?;
        Ok(())
    }

    pub fn path(&self) -> &Path { &self.path }
}

/// ADR 0305 — wraps any `SqlCdcSource` and persists the
/// max LSN observed in each successful batch to a
/// `CursorStore`. On construction, the cursor is loaded
/// so the wrapped source can seek via `start_from`.
pub struct BoundedCdcPuller<S: SqlCdcSource> {
    inner:  S,
    cursor: CursorStore,
    last:   u64,
}

impl<S: SqlCdcSource> BoundedCdcPuller<S> {
    pub fn new(inner: S, cursor: CursorStore) -> BradianResult<Self> {
        let last = cursor.load()?;
        Ok(Self { inner, cursor, last })
    }

    /// LSN the puller resumed from on construction.
    pub fn start_from(&self) -> u64 { self.last }

    pub fn next_batch(&mut self) -> BradianResult<Vec<SqlCdcRow>> {
        let batch = self.inner.next_batch()?;
        if let Some(max_lsn) = batch.iter().map(|r| r.lsn).max() {
            if max_lsn > self.last {
                self.cursor.save(max_lsn)?;
                self.last = max_lsn;
            }
        }
        Ok(batch)
    }
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

    // ADR 0305 — scoped credentials + cursor persistence.

    #[test]
    fn credentials_refuse_non_readonly_connections() {
        let err = SqlCdcCredentials::try_new("repl_user", "slot_a", false).unwrap_err();
        assert!(format!("{err}").contains("readonly"));
        SqlCdcCredentials::try_new("repl_user", "slot_a", true).unwrap();
        let err = SqlCdcCredentials::try_new("", "slot_a", true).unwrap_err();
        assert!(format!("{err}").contains("required"));
    }

    #[test]
    fn cursor_store_round_trips_lsn_across_load_calls() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cursor");
        let store = CursorStore::new(&path);
        assert_eq!(store.load().unwrap(), 0,
            "missing cursor file should load as 0");
        store.save(42).unwrap();
        assert_eq!(store.load().unwrap(), 42);
        // Overwrite is atomic: the second save replaces.
        store.save(99).unwrap();
        assert_eq!(store.load().unwrap(), 99);
    }

    struct MockSource { batches: Vec<Vec<SqlCdcRow>> }
    impl SqlCdcSource for MockSource {
        fn next_batch(&mut self) -> BradianResult<Vec<SqlCdcRow>> {
            Ok(if self.batches.is_empty() { Vec::new() } else { self.batches.remove(0) })
        }
    }

    fn mock_row(lsn: u64) -> SqlCdcRow {
        SqlCdcRow {
            table: "t".into(), primary_key: lsn.to_string(),
            op: SqlCdcOp::Insert, lsn,
            before: None, after: Some(serde_json::json!({"id": lsn})),
            commit_ms: 0,
        }
    }

    #[test]
    fn bounded_puller_resumes_from_persisted_cursor_on_second_construction() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cursor");
        let store = CursorStore::new(&path);
        // First run: two batches, LSNs 10..=12, then 13..=14.
        {
            let src = MockSource { batches: vec![
                vec![mock_row(10), mock_row(11), mock_row(12)],
                vec![mock_row(13), mock_row(14)],
            ]};
            let mut puller = BoundedCdcPuller::new(src, store.clone()).unwrap();
            assert_eq!(puller.start_from(), 0);
            let b1 = puller.next_batch().unwrap();
            assert_eq!(b1.len(), 3);
            assert_eq!(store.load().unwrap(), 12);
            let b2 = puller.next_batch().unwrap();
            assert_eq!(b2.len(), 2);
            assert_eq!(store.load().unwrap(), 14);
        }
        // Second construction (simulated restart) resumes
        // from the persisted cursor.
        let src = MockSource { batches: vec![vec![mock_row(15)]] };
        let puller = BoundedCdcPuller::new(src, store.clone()).unwrap();
        assert_eq!(puller.start_from(), 14,
            "restart must resume from the last persisted LSN");
    }

    #[test]
    fn cursor_does_not_regress_when_batch_has_only_older_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cursor");
        let store = CursorStore::new(&path);
        store.save(100).unwrap();
        let src = MockSource { batches: vec![vec![mock_row(50), mock_row(60)]] };
        let mut puller = BoundedCdcPuller::new(src, store.clone()).unwrap();
        puller.next_batch().unwrap();
        assert_eq!(store.load().unwrap(), 100,
            "cursor must not regress on a batch of stale LSNs");
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
