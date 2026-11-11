//! Dataset completion facts and reporting-rate computation.
//!
//! Spec §8.1 "Preserve correct cell coordinates; do not substitute calculated totals for
//! stored combinations." + G6.md gap #4.
//!
//! Model
//! =====
//!
//! A `CompletionFact` records how many data-set forms were submitted (and how many on time)
//! for an `(dataset, org_unit, period)` cell, alongside the number expected per the dataset's
//! OU assignment set. From this the engine computes the five DHIS2 reporting-rate variants
//! used by indicator expressions:
//!
//! - `REPORTING_RATE = 100 * actual / expected`
//! - `REPORTING_RATE_ON_TIME = 100 * actual_on_time / expected`
//! - `ACTUAL_REPORTS`
//! - `ACTUAL_REPORTS_ON_TIME`
//! - `EXPECTED_REPORTS`
//!
//! Zero expected reports → rate variants resolve to NULL (divide by zero). Count variants
//! return the stored integer; absent facts return NULL.

use ahash::AHashMap;
use dap_core::{Fingerprint, Uid};
use serde::{Deserialize, Serialize};

use crate::expression::{EValue, ReportingRateVariant};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CompletionKey {
    pub dataset: Uid,
    pub org_unit: Uid,
    pub period: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CompletionFact {
    pub key: CompletionKey,
    pub expected: u32,
    pub actual: u32,
    pub actual_on_time: u32,
    /// Source revision used for idempotent ingestion (see `dap-ingest`).
    #[serde(default)]
    pub source_version: u64,
    /// Catalog metadata revision the fact was ingested against. Provenance surfaces this
    /// so a materialized view can decide whether to invalidate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_revision: Option<Fingerprint>,
}

impl CompletionFact {
    pub fn variant(&self, v: ReportingRateVariant) -> EValue {
        use ReportingRateVariant as V;
        match v {
            V::ActualReports => EValue::Number(self.actual as f64),
            V::ActualReportsOnTime => EValue::Number(self.actual_on_time as f64),
            V::ExpectedReports => EValue::Number(self.expected as f64),
            V::ReportingRate => {
                if self.expected == 0 {
                    EValue::Null
                } else {
                    EValue::Number(100.0 * self.actual as f64 / self.expected as f64)
                }
            }
            V::ReportingRateOnTime => {
                if self.expected == 0 {
                    EValue::Null
                } else {
                    EValue::Number(
                        100.0 * self.actual_on_time as f64 / self.expected as f64,
                    )
                }
            }
        }
    }
}

/// In-memory index for completion facts, keyed by `(dataset, org_unit, period)`. The index
/// is thread-safe for reads via interior cloning on lookup.
#[derive(Debug, Default)]
pub struct CompletionStore {
    by_key: AHashMap<CompletionKey, CompletionFact>,
}

impl CompletionStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or supersede a completion fact. Later `source_version` wins per the ingest
    /// version-fence rule (ING-002); equal versions are no-ops.
    pub fn upsert(&mut self, fact: CompletionFact) -> bool {
        match self.by_key.get(&fact.key) {
            Some(existing) if existing.source_version >= fact.source_version => false,
            _ => {
                self.by_key.insert(fact.key.clone(), fact);
                true
            }
        }
    }

    pub fn get(&self, dataset: &Uid, org_unit: &Uid, period: &str) -> Option<CompletionFact> {
        self.by_key
            .get(&CompletionKey {
                dataset: dataset.clone(),
                org_unit: org_unit.clone(),
                period: period.to_string(),
            })
            .cloned()
    }

    /// Resolve a `#{datasetUid.VARIANT}` reference for the current `(OU, period)` context.
    pub fn resolve(
        &self,
        dataset: &Uid,
        org_unit: &Uid,
        period: &str,
        variant: ReportingRateVariant,
    ) -> EValue {
        match self.get(dataset, org_unit, period) {
            Some(fact) => fact.variant(variant),
            None => EValue::Null,
        }
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uid(s: &str) -> Uid {
        Uid::new(s).unwrap()
    }

    fn fact(expected: u32, actual: u32, on_time: u32) -> CompletionFact {
        CompletionFact {
            key: CompletionKey {
                dataset: uid("dsAAAAAAAAA"),
                org_unit: uid("ouAAAAAAAAA"),
                period: "202601".into(),
            },
            expected,
            actual,
            actual_on_time: on_time,
            source_version: 1,
            metadata_revision: None,
        }
    }

    #[test]
    fn computes_five_variants() {
        let f = fact(50, 40, 32);
        assert_eq!(f.variant(ReportingRateVariant::ExpectedReports), EValue::Number(50.0));
        assert_eq!(f.variant(ReportingRateVariant::ActualReports), EValue::Number(40.0));
        assert_eq!(
            f.variant(ReportingRateVariant::ActualReportsOnTime),
            EValue::Number(32.0)
        );
        assert_eq!(
            f.variant(ReportingRateVariant::ReportingRate),
            EValue::Number(80.0)
        );
        assert_eq!(
            f.variant(ReportingRateVariant::ReportingRateOnTime),
            EValue::Number(64.0)
        );
    }

    #[test]
    fn zero_expected_yields_null_rate() {
        let f = fact(0, 0, 0);
        assert_eq!(f.variant(ReportingRateVariant::ReportingRate), EValue::Null);
        assert_eq!(
            f.variant(ReportingRateVariant::ReportingRateOnTime),
            EValue::Null
        );
    }

    #[test]
    fn store_resolves_missing_as_null() {
        let store = CompletionStore::new();
        assert_eq!(
            store.resolve(
                &uid("dsAAAAAAAAA"),
                &uid("ouAAAAAAAAA"),
                "202601",
                ReportingRateVariant::ReportingRate,
            ),
            EValue::Null
        );
    }

    #[test]
    fn upsert_version_fence() {
        let mut store = CompletionStore::new();
        let mut f1 = fact(50, 40, 32);
        f1.source_version = 5;
        assert!(store.upsert(f1));
        let mut stale = fact(999, 999, 999);
        stale.source_version = 3;
        assert!(!store.upsert(stale));
        let got = store
            .get(&uid("dsAAAAAAAAA"), &uid("ouAAAAAAAAA"), "202601")
            .unwrap();
        assert_eq!(got.actual, 40);
    }
}
