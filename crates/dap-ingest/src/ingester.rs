//! Idempotent ingestion: apply canonical changes to a `Partition` with version fencing and
//! quarantine for invalid payloads.

use std::sync::Arc;

use dap_core::{DapError, DapResult, Value};
use dap_storage::Partition;
use serde::Serialize;

use crate::envelope::{Change, Operation};
use crate::quarantine::{QuarantineLog, QuarantinedChange};

#[derive(Clone, Debug, Default, Serialize)]
pub struct IngestReport {
    pub accepted: u64,
    pub dropped_stale: u64,
    pub deletes: u64,
    pub quarantined: u64,
}

#[derive(Debug)]
pub struct Ingester {
    partition: Arc<Partition>,
    quarantine: QuarantineLog,
    /// Columns composing the primary key, by name. Must be a prefix-disjoint subset of the
    /// partition schema.
    primary_key: Vec<String>,
}

impl Ingester {
    pub fn new(
        partition: Arc<Partition>,
        quarantine: QuarantineLog,
        primary_key: Vec<String>,
    ) -> Self {
        Self {
            partition,
            quarantine,
            primary_key,
        }
    }

    pub fn apply(&self, change: &Change) -> DapResult<Outcome> {
        match change.operation {
            Operation::Upsert => {
                let schema = self.partition.schema();
                let row = match change.row_for_schema(schema) {
                    Ok(r) => r,
                    Err(e) => {
                        self.quarantine_change(change, &e)?;
                        return Ok(Outcome::Quarantined);
                    }
                };
                self.partition.upsert_row(row, change.source_version)?;
                Ok(Outcome::Accepted)
            }
            Operation::Delete => {
                let key_values = self.extract_key(change)?;
                self.partition.delete_row(key_values, change.source_version)?;
                Ok(Outcome::Deleted)
            }
        }
    }

    pub fn apply_batch(&self, changes: &[Change]) -> DapResult<IngestReport> {
        let mut report = IngestReport::default();
        for c in changes {
            match self.apply(c) {
                Ok(Outcome::Accepted) => report.accepted += 1,
                Ok(Outcome::Deleted) => report.deletes += 1,
                Ok(Outcome::Stale) => report.dropped_stale += 1,
                Ok(Outcome::Quarantined) => report.quarantined += 1,
                Err(e) => {
                    tracing::warn!(change_id = %c.change_id, error = %e, "ingest error -> quarantine");
                    self.quarantine_change(c, &e)?;
                    report.quarantined += 1;
                }
            }
        }
        Ok(report)
    }

    fn extract_key(&self, change: &Change) -> DapResult<Vec<Value>> {
        let obj = change.payload.as_object().ok_or_else(|| {
            DapError::invalid_value("delete payload must contain primary key fields")
        })?;
        let mut key = Vec::with_capacity(self.primary_key.len());
        for col_name in &self.primary_key {
            let v = obj.get(col_name).cloned().ok_or_else(|| {
                DapError::invalid_value(format!(
                    "delete payload missing primary-key column '{col_name}'"
                ))
            })?;
            // Convert minimally — strings and numbers both allowed as PK. For v1 we accept
            // JSON strings as `Value::Str` and numbers as `Value::I64`.
            let v = match v {
                serde_json::Value::String(s) => Value::Str(s),
                serde_json::Value::Number(n) => Value::I64(n.as_i64().ok_or_else(|| {
                    DapError::invalid_value("PK number must be integer")
                })?),
                other => {
                    return Err(DapError::invalid_value(format!(
                        "unsupported PK value: {other}"
                    )))
                }
            };
            key.push(v);
        }
        Ok(key)
    }

    fn quarantine_change(&self, change: &Change, err: &DapError) -> DapResult<()> {
        let q = QuarantinedChange {
            change_id: change.change_id.clone(),
            reason: err.to_string(),
            payload: change.payload.clone(),
            received_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
        };
        self.quarantine.append(&q)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Accepted,
    Deleted,
    Stale,
    Quarantined,
}

#[cfg(test)]
mod tests {
    use super::*;
    use dap_core::{Column, LogicalType, Schema};
    use dap_storage::PartitionOptions;
    use tempfile::tempdir;

    fn schema() -> Schema {
        Schema::new(vec![
            Column::new("ou", LogicalType::Uid).required(),
            Column::new("period", LogicalType::Period).required(),
            Column::new("de", LogicalType::Uid).required(),
            Column::new("value", LogicalType::F64),
        ])
        .unwrap()
    }

    fn open(root: &std::path::Path) -> Arc<Partition> {
        Partition::open(
            root,
            PartitionOptions {
                dataset_id: "d".into(),
                partition_id: "p".into(),
                schema: schema(),
                primary_key_columns: vec!["ou".into(), "period".into(), "de".into()],
                flush_rows: 1024,
                flush_bytes: 1 << 24,
            },
        )
        .unwrap()
    }

    #[test]
    fn upsert_then_delete_change_pair() {
        let dir = tempdir().unwrap();
        let part = open(dir.path());
        let q = QuarantineLog::open(dir.path()).unwrap();
        let ing = Ingester::new(
            part.clone(),
            q,
            vec!["ou".into(), "period".into(), "de".into()],
        );

        let upsert = Change {
            change_id: "c1".into(),
            site_id: None,
            object_type: "AGGREGATE_DATA_VALUE".into(),
            object_uid: dap_core::Uid::new("deAAAAAAAAA").unwrap(),
            source_version: 1,
            operation: Operation::Upsert,
            occurred_at: "2026-01-01T00:00:00Z".into(),
            metadata_revision: None,
            payload_hash: None,
            payload: serde_json::json!({
                "ou": "ouAAAAAAAAA",
                "period": "202601",
                "de": "deAAAAAAAAA",
                "value": 42.0,
            }),
        };
        assert_eq!(ing.apply(&upsert).unwrap(), Outcome::Accepted);

        let delete = Change {
            operation: Operation::Delete,
            source_version: 2,
            change_id: "c2".into(),
            ..upsert.clone()
        };
        assert_eq!(ing.apply(&delete).unwrap(), Outcome::Deleted);
    }

    #[test]
    fn invalid_payload_is_quarantined() {
        let dir = tempdir().unwrap();
        let part = open(dir.path());
        let q = QuarantineLog::open(dir.path()).unwrap();
        let ing = Ingester::new(
            part.clone(),
            q,
            vec!["ou".into(), "period".into(), "de".into()],
        );
        let bad = Change {
            change_id: "c1".into(),
            site_id: None,
            object_type: "AGGREGATE_DATA_VALUE".into(),
            object_uid: dap_core::Uid::new("deAAAAAAAAA").unwrap(),
            source_version: 1,
            operation: Operation::Upsert,
            occurred_at: "now".into(),
            metadata_revision: None,
            payload_hash: None,
            // Period "bad" is not valid.
            payload: serde_json::json!({
                "ou": "ouAAAAAAAAA",
                "period": "notaperiod",
                "de": "deAAAAAAAAA",
                "value": 1.0,
            }),
        };
        assert_eq!(ing.apply(&bad).unwrap(), Outcome::Quarantined);
    }

    #[test]
    fn stale_replay_dropped_but_reported_accepted() {
        // v1 Ingester always forwards to Partition::upsert_row which internally drops stale
        // versions. The Ingester reports `Accepted` because the WAL write occurred and the
        // storage layer's fence rejected the overwrite — a legitimate idempotent outcome.
        let dir = tempdir().unwrap();
        let part = open(dir.path());
        let q = QuarantineLog::open(dir.path()).unwrap();
        let ing = Ingester::new(
            part.clone(),
            q,
            vec!["ou".into(), "period".into(), "de".into()],
        );
        let fresh = Change {
            change_id: "c1".into(),
            site_id: None,
            object_type: "AGGREGATE_DATA_VALUE".into(),
            object_uid: dap_core::Uid::new("deAAAAAAAAA").unwrap(),
            source_version: 10,
            operation: Operation::Upsert,
            occurred_at: "2026-01-01T00:00:00Z".into(),
            metadata_revision: None,
            payload_hash: None,
            payload: serde_json::json!({
                "ou": "ouAAAAAAAAA",
                "period": "202601",
                "de": "deAAAAAAAAA",
                "value": 99.0,
            }),
        };
        ing.apply(&fresh).unwrap();
        let stale = Change {
            source_version: 5,
            change_id: "c2".into(),
            payload: serde_json::json!({
                "ou": "ouAAAAAAAAA",
                "period": "202601",
                "de": "deAAAAAAAAA",
                "value": 1.0,
            }),
            ..fresh.clone()
        };
        ing.apply(&stale).unwrap();

        let got = part
            .get(&dap_storage::PrimaryKey(vec![
                Value::Str("ouAAAAAAAAA".into()),
                Value::Str("202601".into()),
                Value::Str("deAAAAAAAAA".into()),
            ]))
            .unwrap()
            .unwrap();
        assert_eq!(got.0[3], Value::F64(99.0));
    }
}
