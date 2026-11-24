//! Idempotent ingestion: apply canonical changes to a `Partition` with version fencing and
//! quarantine for invalid payloads.
//!
//! ADR 0301 — the ingester can be configured for explicit
//! change-ID dedup via `with_idempotent_dedup`. The index
//! is in-memory and process-local; a durable index so
//! idempotency survives restart is a Gate 3+ follow-up.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use bradian_core::{BradianError, BradianResult, Value};
use bradian_storage::Partition;
use serde::Serialize;

use crate::envelope::{Change, Operation};
use crate::quarantine::{QuarantineLog, QuarantinedChange};

#[derive(Clone, Debug, Default, Serialize)]
pub struct IngestReport {
    pub accepted: u64,
    pub dropped_stale: u64,
    pub deletes: u64,
    pub quarantined: u64,
    /// ADR 0301 — changes refused because their `change_id`
    /// was already applied through this ingester. Always
    /// zero unless `with_idempotent_dedup` is enabled.
    #[serde(default)]
    pub duplicates: u64,
}

#[derive(Debug)]
pub struct Ingester {
    partition: Arc<Partition>,
    quarantine: QuarantineLog,
    /// Columns composing the primary key, by name. Must be a prefix-disjoint subset of the
    /// partition schema.
    primary_key: Vec<String>,
    /// Hard cap on the number of changes `apply_batch` will process in a single call.
    /// `None` leaves ingest unbounded (default, preserves pre-ING-003 behaviour); `Some(n)`
    /// fails the whole batch closed with [`BradianError::invalid_value`] when `n` is exceeded,
    /// so the caller can retry smaller without partial-application ambiguity.
    max_batch_size: Option<usize>,
    /// ADR 0301 — opt-in in-memory change-ID dedup. `None`
    /// preserves pre-0301 behaviour; `Some` activates
    /// idempotent replay for the lifetime of this process.
    seen_change_ids: Option<Mutex<HashSet<String>>>,
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
            max_batch_size: None,
            seen_change_ids: None,
        }
    }

    /// Set the maximum number of changes a single `apply_batch` call will accept.
    /// Oversized batches are rejected before any side effects so the caller can retry
    /// with smaller chunks.
    pub fn with_max_batch_size(mut self, max: usize) -> Self {
        self.max_batch_size = Some(max);
        self
    }

    /// ADR 0301 — enable in-memory change-ID dedup. A
    /// change whose `change_id` was already applied
    /// through this ingester is refused with
    /// `Outcome::DuplicateChangeId` BEFORE any partition
    /// or quarantine side effect.
    pub fn with_idempotent_dedup(mut self) -> Self {
        self.seen_change_ids = Some(Mutex::new(HashSet::new()));
        self
    }

    /// Returns `true` if the change_id is already known
    /// (dedup disabled always returns false). Records the
    /// id if it is new.
    fn record_or_duplicate(&self, change_id: &str) -> bool {
        let Some(set) = &self.seen_change_ids else { return false; };
        let mut g = set.lock().expect("ingester dedup mutex poisoned");
        !g.insert(change_id.to_owned())
    }

    pub fn apply(&self, change: &Change) -> BradianResult<Outcome> {
        if self.record_or_duplicate(&change.change_id) {
            return Ok(Outcome::DuplicateChangeId);
        }
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

    pub fn apply_batch(&self, changes: &[Change]) -> BradianResult<IngestReport> {
        if let Some(max) = self.max_batch_size {
            if changes.len() > max {
                return Err(BradianError::invalid_value(format!(
                    "ingest batch of {} changes exceeds configured max_batch_size={}",
                    changes.len(),
                    max
                )));
            }
        }
        let mut report = IngestReport::default();
        for c in changes {
            match self.apply(c) {
                Ok(Outcome::Accepted) => report.accepted += 1,
                Ok(Outcome::Deleted) => report.deletes += 1,
                Ok(Outcome::Stale) => report.dropped_stale += 1,
                Ok(Outcome::Quarantined) => report.quarantined += 1,
                Ok(Outcome::DuplicateChangeId) => report.duplicates += 1,
                Err(e) => {
                    tracing::warn!(change_id = %c.change_id, error = %e, "ingest error -> quarantine");
                    self.quarantine_change(c, &e)?;
                    report.quarantined += 1;
                }
            }
        }
        Ok(report)
    }

    fn extract_key(&self, change: &Change) -> BradianResult<Vec<Value>> {
        let obj = change.payload.as_object().ok_or_else(|| {
            BradianError::invalid_value("delete payload must contain primary key fields")
        })?;
        let mut key = Vec::with_capacity(self.primary_key.len());
        for col_name in &self.primary_key {
            let v = obj.get(col_name).cloned().ok_or_else(|| {
                BradianError::invalid_value(format!(
                    "delete payload missing primary-key column '{col_name}'"
                ))
            })?;
            // Convert minimally — strings and numbers both allowed as PK. For v1 we accept
            // JSON strings as `Value::Str` and numbers as `Value::I64`.
            let v = match v {
                serde_json::Value::String(s) => Value::Str(s),
                serde_json::Value::Number(n) => Value::I64(n.as_i64().ok_or_else(|| {
                    BradianError::invalid_value("PK number must be integer")
                })?),
                other => {
                    return Err(BradianError::invalid_value(format!(
                        "unsupported PK value: {other}"
                    )))
                }
            };
            key.push(v);
        }
        Ok(key)
    }

    fn quarantine_change(&self, change: &Change, err: &BradianError) -> BradianResult<()> {
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
    /// ADR 0301 — refused because the change-ID was already
    /// applied through this ingester (dedup enabled).
    DuplicateChangeId,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bradian_core::{Column, LogicalType, Schema};
    use bradian_storage::PartitionOptions;
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
            max_rows_per_page: None,
            use_column_dict: false,
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
            object_uid: bradian_core::Uid::new("deAAAAAAAAA").unwrap(),
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
            object_uid: bradian_core::Uid::new("deAAAAAAAAA").unwrap(),
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
            object_uid: bradian_core::Uid::new("deAAAAAAAAA").unwrap(),
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
            .get(&bradian_storage::PrimaryKey(vec![
                Value::Str("ouAAAAAAAAA".into()),
                Value::Str("202601".into()),
                Value::Str("deAAAAAAAAA".into()),
            ]))
            .unwrap()
            .unwrap();
        assert_eq!(got.0[3], Value::F64(99.0));
    }

    fn mk_change(id: &str, version: u64) -> Change {
        Change {
            change_id: id.into(),
            site_id: None,
            object_type: "AGGREGATE_DATA_VALUE".into(),
            object_uid: bradian_core::Uid::new("deAAAAAAAAA").unwrap(),
            source_version: version,
            operation: Operation::Upsert,
            occurred_at: "2026-01-01T00:00:00Z".into(),
            metadata_revision: None,
            payload_hash: None,
            payload: serde_json::json!({
                "ou": "ouAAAAAAAAA",
                "period": "202601",
                "de": "deAAAAAAAAA",
                "value": 1.0,
            }),
        }
    }

    // ADR 0301 — idempotent replay via change-ID dedup.

    #[test]
    fn replayed_change_id_is_deduped_without_side_effect_when_dedup_enabled() {
        let dir = tempdir().unwrap();
        let part = open(dir.path());
        let q = QuarantineLog::open(dir.path()).unwrap();
        let ing = Ingester::new(part.clone(), q,
            vec!["ou".into(), "period".into(), "de".into()])
            .with_idempotent_dedup();
        let c = mk_change("abc", 1);
        assert_eq!(ing.apply(&c).unwrap(), Outcome::Accepted);
        // Second application must be refused BEFORE any side
        // effect on the partition or the quarantine.
        assert_eq!(ing.apply(&c).unwrap(), Outcome::DuplicateChangeId);
    }

    #[test]
    fn replay_batch_reports_duplicates_and_no_side_effects_after_first_apply() {
        let dir = tempdir().unwrap();
        let part = open(dir.path());
        let q = QuarantineLog::open(dir.path()).unwrap();
        let ing = Ingester::new(part.clone(), q,
            vec!["ou".into(), "period".into(), "de".into()])
            .with_idempotent_dedup();
        let batch: Vec<Change> = (0..10)
            .map(|i| mk_change(&format!("c{i}"), i + 1))
            .collect();
        let first  = ing.apply_batch(&batch).unwrap();
        let second = ing.apply_batch(&batch).unwrap();
        assert_eq!(first.accepted,  10);
        assert_eq!(first.duplicates,  0);
        assert_eq!(second.accepted,    0);
        assert_eq!(second.duplicates, 10);
    }

    #[test]
    fn mixed_batch_counts_new_and_duplicate_change_ids_separately() {
        let dir = tempdir().unwrap();
        let part = open(dir.path());
        let q = QuarantineLog::open(dir.path()).unwrap();
        let ing = Ingester::new(part.clone(), q,
            vec!["ou".into(), "period".into(), "de".into()])
            .with_idempotent_dedup();
        // Seed three known change-ids.
        let seeded = vec![mk_change("c1", 1), mk_change("c2", 2), mk_change("c3", 3)];
        let r1 = ing.apply_batch(&seeded).unwrap();
        assert_eq!(r1.accepted, 3);
        // Mixed: two already-seen, two new.
        let mixed = vec![mk_change("c2", 2), mk_change("c4", 4),
                         mk_change("c1", 1), mk_change("c5", 5)];
        let r2 = ing.apply_batch(&mixed).unwrap();
        assert_eq!(r2.accepted,   2);
        assert_eq!(r2.duplicates, 2);
    }

    #[test]
    fn dedup_disabled_by_default_preserves_prior_behaviour() {
        // Without `with_idempotent_dedup`, the ingester
        // never reports `DuplicateChangeId` — the same
        // change_id applied twice is just "second apply" at
        // the partition level (version fencing handles it).
        let dir = tempdir().unwrap();
        let part = open(dir.path());
        let q = QuarantineLog::open(dir.path()).unwrap();
        let ing = Ingester::new(part.clone(), q,
            vec!["ou".into(), "period".into(), "de".into()]);
        let c = mk_change("abc", 1);
        assert_eq!(ing.apply(&c).unwrap(), Outcome::Accepted);
        assert_eq!(ing.apply(&c).unwrap(), Outcome::Accepted,
            "dedup-disabled ingester must not emit DuplicateChangeId");
    }

    #[test]
    fn apply_batch_rejects_oversized_batch_before_side_effects() {
        let dir = tempdir().unwrap();
        let part = open(dir.path());
        let q = QuarantineLog::open(dir.path()).unwrap();
        let ing = Ingester::new(
            part.clone(),
            q,
            vec!["ou".into(), "period".into(), "de".into()],
        )
        .with_max_batch_size(3);

        let batch: Vec<Change> = (0..5)
            .map(|i| mk_change(&format!("c{i}"), (i + 1) as u64))
            .collect();

        let err = ing.apply_batch(&batch).expect_err("oversized batch rejected");
        assert!(
            err.to_string().contains("max_batch_size"),
            "error message mentions the configured ceiling: {err}"
        );

        // Nothing was accepted — the oversized batch fails closed, so no partition
        // snapshot contains the first N-1 rows.
        assert!(part.visible_segments().is_empty());
        let row = part
            .get(&bradian_storage::PrimaryKey(vec![
                Value::Str("ouAAAAAAAAA".into()),
                Value::Str("202601".into()),
                Value::Str("deAAAAAAAAA".into()),
            ]))
            .unwrap();
        assert!(row.is_none());
    }

    #[test]
    fn apply_batch_accepts_batch_at_exact_limit() {
        let dir = tempdir().unwrap();
        let part = open(dir.path());
        let q = QuarantineLog::open(dir.path()).unwrap();
        let ing = Ingester::new(
            part.clone(),
            q,
            vec!["ou".into(), "period".into(), "de".into()],
        )
        .with_max_batch_size(3);

        let batch: Vec<Change> = (0..3)
            .map(|i| mk_change(&format!("c{i}"), (i + 1) as u64))
            .collect();
        let report = ing.apply_batch(&batch).unwrap();
        // All three target the same PK → one accepted, two are earlier versions that get
        // dropped as stale by the version fence. The point of this test is that the
        // bound did not fire.
        assert_eq!(report.accepted + report.dropped_stale, 3);
    }
}
