//! Incremental refresh for materialised views (INC-001, INC-002, INC-003).
//!
//! When upstream data changes, a materialised view can be refreshed without re-reading the
//! full upstream fact set, provided the refresh algorithm knows:
//!
//! 1. The *old* and *new* value of each affected `(group_by)` key so it can retract the
//!    stale contribution before applying the new one. DHIS2 calls this "old/new
//!    contribution".
//! 2. The *shape* of the aggregate (additive / holistic). Additive aggregates (SUM, COUNT)
//!    retract cleanly by subtraction; AVG retracts via `(sum, count)` pairs; MIN / MAX /
//!    COUNT DISTINCT / percentile require a full recompute of the affected cells because
//!    they are not invertible. This module encodes those semantics.
//!
//! The module is intentionally simple — it operates on a per-key `AggCell` value and does
//! not touch segment files directly. Callers plug it into the `MaterializedView` state via
//! `apply_change`.

use ahash::AHashMap;
use dap_core::{DapError, DapResult, Value};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AggKind {
    /// Invertible on numeric values: new = old + delta.
    Sum,
    /// Invertible on counters: new = old + delta_count.
    Count,
    /// Invertible via two-moment state: new = (old.sum ± delta, old.count ± 1).
    Avg,
    /// NOT invertible: refresh must re-scan the affected group.
    Min,
    /// NOT invertible: refresh must re-scan the affected group.
    Max,
    /// NOT invertible: refresh must re-scan the affected group.
    CountDistinct,
}

impl AggKind {
    pub fn is_invertible(&self) -> bool {
        matches!(self, AggKind::Sum | AggKind::Count | AggKind::Avg)
    }
}

/// Per-key aggregate cell. Carries the running state so a change can be retracted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AggCell {
    Sum { sum: f64 },
    Count { count: i64 },
    Avg { sum: f64, count: i64 },
    /// Opaque "needs full recompute" marker — carries the last known finalized value for
    /// read serving, but the refresh algorithm must re-scan whenever an edit touches the
    /// key.
    NonInvertible { last_value: Option<f64>, dirty: bool },
}

impl AggCell {
    pub fn new(kind: AggKind) -> Self {
        match kind {
            AggKind::Sum => AggCell::Sum { sum: 0.0 },
            AggKind::Count => AggCell::Count { count: 0 },
            AggKind::Avg => AggCell::Avg { sum: 0.0, count: 0 },
            AggKind::Min | AggKind::Max | AggKind::CountDistinct => AggCell::NonInvertible {
                last_value: None,
                dirty: true,
            },
        }
    }

    /// Apply an edit: `old_value` is what the source held before the change (`None` on
    /// insert), `new_value` is what the source holds after (`None` on delete).
    pub fn apply_change(
        &mut self,
        kind: AggKind,
        old_value: Option<f64>,
        new_value: Option<f64>,
    ) -> DapResult<()> {
        match (kind, self) {
            (AggKind::Sum, AggCell::Sum { sum }) => {
                if let Some(o) = old_value {
                    *sum -= o;
                }
                if let Some(n) = new_value {
                    *sum += n;
                }
                Ok(())
            }
            (AggKind::Count, AggCell::Count { count }) => {
                if old_value.is_some() {
                    *count -= 1;
                }
                if new_value.is_some() {
                    *count += 1;
                }
                Ok(())
            }
            (AggKind::Avg, AggCell::Avg { sum, count }) => {
                if let Some(o) = old_value {
                    *sum -= o;
                    *count -= 1;
                }
                if let Some(n) = new_value {
                    *sum += n;
                    *count += 1;
                }
                Ok(())
            }
            (AggKind::Min | AggKind::Max | AggKind::CountDistinct, AggCell::NonInvertible { dirty, .. }) => {
                *dirty = true;
                Ok(())
            }
            (kind, cell) => Err(DapError::invariant(format!(
                "AggCell::apply_change: kind {kind:?} does not match cell {cell:?}"
            ))),
        }
    }

    pub fn finalize(&self) -> Option<f64> {
        match self {
            AggCell::Sum { sum } => Some(*sum),
            AggCell::Count { count } => Some(*count as f64),
            AggCell::Avg { sum, count } => {
                if *count > 0 {
                    Some(sum / *count as f64)
                } else {
                    None
                }
            }
            AggCell::NonInvertible { last_value, .. } => *last_value,
        }
    }

    pub fn needs_recompute(&self) -> bool {
        matches!(self, AggCell::NonInvertible { dirty: true, .. })
    }

    pub fn mark_fresh(&mut self, value: Option<f64>) {
        if let AggCell::NonInvertible { last_value, dirty } = self {
            *last_value = value;
            *dirty = false;
        }
    }
}

/// An incremental aggregate state keyed by group-by tuple. The map is optimised for the
/// case where most edits touch a small subset of keys relative to the whole view.
#[derive(Debug, Clone)]
pub struct IncrementalAggregate {
    kind: AggKind,
    cells: AHashMap<Vec<Value>, AggCell>,
    /// Keys that touched a non-invertible aggregate and must be re-scanned next refresh.
    dirty_keys: ahash::AHashSet<Vec<Value>>,
}

impl IncrementalAggregate {
    pub fn new(kind: AggKind) -> Self {
        Self {
            kind,
            cells: AHashMap::default(),
            dirty_keys: ahash::AHashSet::default(),
        }
    }

    pub fn is_invertible(&self) -> bool {
        self.kind.is_invertible()
    }

    pub fn apply_change(
        &mut self,
        key: Vec<Value>,
        old_value: Option<f64>,
        new_value: Option<f64>,
    ) -> DapResult<()> {
        let cell = self
            .cells
            .entry(key.clone())
            .or_insert_with(|| AggCell::new(self.kind));
        cell.apply_change(self.kind, old_value, new_value)?;
        if cell.needs_recompute() {
            self.dirty_keys.insert(key);
        }
        Ok(())
    }

    /// Finalise a snapshot of the view: `(key, value)` pairs. Non-invertible aggregates
    /// that are still dirty surface as `None` — the host must call `mark_fresh` once it
    /// has recomputed the group.
    pub fn snapshot(&self) -> Vec<(Vec<Value>, Option<f64>)> {
        self.cells
            .iter()
            .map(|(k, c)| (k.clone(), c.finalize()))
            .collect()
    }

    pub fn dirty_keys(&self) -> &ahash::AHashSet<Vec<Value>> {
        &self.dirty_keys
    }

    pub fn mark_fresh(&mut self, key: &[Value], value: Option<f64>) {
        if let Some(cell) = self.cells.get_mut(key) {
            cell.mark_fresh(value);
        }
        self.dirty_keys.remove(key);
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dap_core::Value;

    fn key(ou: &str, period: &str) -> Vec<Value> {
        vec![
            Value::Str(ou.to_string()),
            Value::Str(period.to_string()),
        ]
    }

    #[test]
    fn sum_insert_update_delete() {
        let mut agg = IncrementalAggregate::new(AggKind::Sum);
        agg.apply_change(key("ouA", "202601"), None, Some(10.0)).unwrap();
        agg.apply_change(key("ouA", "202601"), None, Some(5.0)).unwrap();
        agg.apply_change(key("ouA", "202601"), Some(5.0), Some(7.0))
            .unwrap();
        // Current: 10 + 7 = 17.
        let snap: AHashMap<_, _> = agg.snapshot().into_iter().collect();
        assert_eq!(snap[&key("ouA", "202601")], Some(17.0));

        agg.apply_change(key("ouA", "202601"), Some(7.0), None).unwrap();
        let snap: AHashMap<_, _> = agg.snapshot().into_iter().collect();
        assert_eq!(snap[&key("ouA", "202601")], Some(10.0));
    }

    #[test]
    fn avg_retracts_sum_and_count() {
        let mut agg = IncrementalAggregate::new(AggKind::Avg);
        agg.apply_change(key("ouA", "202601"), None, Some(10.0)).unwrap();
        agg.apply_change(key("ouA", "202601"), None, Some(20.0)).unwrap();
        let snap: AHashMap<_, _> = agg.snapshot().into_iter().collect();
        assert_eq!(snap[&key("ouA", "202601")], Some(15.0));

        // Delete the 20 -> avg must become 10.
        agg.apply_change(key("ouA", "202601"), Some(20.0), None).unwrap();
        let snap: AHashMap<_, _> = agg.snapshot().into_iter().collect();
        assert_eq!(snap[&key("ouA", "202601")], Some(10.0));
    }

    #[test]
    fn min_requires_recompute_and_mark_fresh() {
        let mut agg = IncrementalAggregate::new(AggKind::Min);
        assert!(!agg.is_invertible());
        agg.apply_change(key("ouA", "202601"), None, Some(10.0)).unwrap();
        assert_eq!(agg.dirty_keys().len(), 1, "any change dirties the key");
        // Caller re-scans the group and marks it fresh.
        agg.mark_fresh(&key("ouA", "202601"), Some(10.0));
        assert_eq!(agg.dirty_keys().len(), 0);
        let snap: AHashMap<_, _> = agg.snapshot().into_iter().collect();
        assert_eq!(snap[&key("ouA", "202601")], Some(10.0));
    }

    #[test]
    fn count_tracks_insert_delete_balance() {
        let mut agg = IncrementalAggregate::new(AggKind::Count);
        for _ in 0..5 {
            agg.apply_change(key("ouA", "202601"), None, Some(1.0)).unwrap();
        }
        agg.apply_change(key("ouA", "202601"), Some(1.0), None).unwrap();
        let snap: AHashMap<_, _> = agg.snapshot().into_iter().collect();
        assert_eq!(snap[&key("ouA", "202601")], Some(4.0));
    }
}
