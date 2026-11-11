//! In-memory mutable delta ("memtable") — row-level upserts and deletes keyed by the
//! dataset-defined primary key, versioned by monotonic source revision.

use std::collections::BTreeMap;

use dap_core::Value;
use serde::{Deserialize, Serialize};

/// A row's primary key. Serialised as bincode so the WAL/stream path is cheap.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PrimaryKey(pub Vec<Value>);

/// A row's cell payload. The ordering matches the dataset schema.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row(pub Vec<Value>);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeltaEntry {
    /// `None` = tombstone (delete).
    pub row: Option<Row>,
    /// Source-system revision: monotonic per key.
    pub source_version: u64,
    /// Last applied WAL LSN (useful for reconciliation / watermarks).
    pub lsn: u64,
}

impl DeltaEntry {
    pub fn is_tombstone(&self) -> bool {
        self.row.is_none()
    }
}

#[derive(Debug, Default, Clone)]
pub struct Delta {
    entries: BTreeMap<PrimaryKey, DeltaEntry>,
    bytes_estimate: usize,
}

impl Delta {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply an upsert. Returns `true` iff the write took effect (not dropped by version fence).
    pub fn upsert(&mut self, key: PrimaryKey, row: Row, source_version: u64, lsn: u64) -> bool {
        self.apply(key, Some(row), source_version, lsn)
    }

    /// Apply a delete. Returns `true` iff the delete took effect.
    pub fn delete(&mut self, key: PrimaryKey, source_version: u64, lsn: u64) -> bool {
        self.apply(key, None, source_version, lsn)
    }

    fn apply(
        &mut self,
        key: PrimaryKey,
        row: Option<Row>,
        source_version: u64,
        lsn: u64,
    ) -> bool {
        match self.entries.get(&key) {
            Some(existing) if existing.source_version >= source_version => false,
            _ => {
                let bytes =
                    estimate_bytes(&key) + row.as_ref().map(estimate_row_bytes).unwrap_or(0);
                self.bytes_estimate = self.bytes_estimate.saturating_add(bytes);
                self.entries.insert(
                    key,
                    DeltaEntry {
                        row,
                        source_version,
                        lsn,
                    },
                );
                true
            }
        }
    }

    pub fn get(&self, key: &PrimaryKey) -> Option<&DeltaEntry> {
        self.entries.get(key)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn bytes_estimate(&self) -> usize {
        self.bytes_estimate
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes_estimate = 0;
    }

    /// Iterator over `(key, entry)` in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&PrimaryKey, &DeltaEntry)> {
        self.entries.iter()
    }

    /// Consume self and return a sorted vector of entries. Used by compaction.
    pub fn into_sorted_vec(self) -> Vec<(PrimaryKey, DeltaEntry)> {
        self.entries.into_iter().collect()
    }
}

fn estimate_bytes(key: &PrimaryKey) -> usize {
    16 + key.0.iter().map(estimate_value_bytes).sum::<usize>()
}

fn estimate_row_bytes(row: &Row) -> usize {
    16 + row.0.iter().map(estimate_value_bytes).sum::<usize>()
}

fn estimate_value_bytes(v: &Value) -> usize {
    match v {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::I64(_) => 8,
        Value::F64(_) => 8,
        Value::Decimal128 { .. } => 17,
        Value::Str(s) => s.len() + 4,
        Value::Uid(u) => u.as_str().len() + 4,
        Value::Timestamp(_) => 8,
        Value::Period(p) => p.len() + 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(v: i64) -> PrimaryKey {
        PrimaryKey(vec![Value::I64(v)])
    }
    fn row(v: i64) -> Row {
        Row(vec![Value::I64(v)])
    }

    #[test]
    fn version_fence_rejects_stale_upsert() {
        let mut d = Delta::new();
        assert!(d.upsert(pk(1), row(10), 5, 100));
        assert!(!d.upsert(pk(1), row(99), 3, 101), "stale version dropped");
        assert_eq!(d.get(&pk(1)).unwrap().row.as_ref().unwrap().0, vec![Value::I64(10)]);
    }

    #[test]
    fn upsert_delete_sequence_converges() {
        let mut d = Delta::new();
        assert!(d.upsert(pk(1), row(1), 1, 10));
        assert!(d.upsert(pk(1), row(2), 2, 11));
        assert!(d.delete(pk(1), 3, 12));
        assert!(d.get(&pk(1)).unwrap().is_tombstone());
    }

    #[test]
    fn same_version_not_applied_twice() {
        let mut d = Delta::new();
        assert!(d.upsert(pk(1), row(1), 1, 10));
        assert!(!d.upsert(pk(1), row(2), 1, 11), "same version dropped");
    }

    #[test]
    fn delete_then_newer_upsert_resurrects() {
        let mut d = Delta::new();
        assert!(d.delete(pk(1), 5, 10));
        assert!(d.upsert(pk(1), row(7), 6, 11));
        assert!(!d.get(&pk(1)).unwrap().is_tombstone());
    }
}
