//! ADR 0116 — in-process metrics registry (counters +
//! histograms).
//!
//! Lightweight registry for operator-facing diagnostics.
//! Thread-safe via `parking_lot::RwLock` + atomic counters.
//! Does NOT ship an HTTP endpoint or external scraper
//! integration — those live in `bradian-serving`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;

/// A monotone u64 counter.
#[derive(Debug, Default)]
pub struct Counter {
    value: AtomicU64,
}

impl Counter {
    pub fn increment(&self) { self.value.fetch_add(1, Ordering::Relaxed); }
    pub fn add(&self, n: u64) { self.value.fetch_add(n, Ordering::Relaxed); }
    pub fn get(&self) -> u64 { self.value.load(Ordering::Relaxed) }
}

/// Simple fixed-bucket histogram. Buckets are
/// upper-bounds. Operator picks the bucket layout at
/// registration.
#[derive(Debug)]
pub struct Histogram {
    buckets: Vec<u64>,         // upper bounds (ascending)
    counts: Vec<AtomicU64>,    // same length as buckets
    overflow: AtomicU64,       // samples exceeding the last bucket
    sum: AtomicU64,
    count: AtomicU64,
}

impl Histogram {
    pub fn new(buckets: Vec<u64>) -> Self {
        let n = buckets.len();
        Self {
            buckets,
            counts: (0..n).map(|_| AtomicU64::new(0)).collect(),
            overflow: AtomicU64::new(0),
            sum: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    pub fn observe(&self, sample: u64) {
        self.sum.fetch_add(sample, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        for (i, bound) in self.buckets.iter().enumerate() {
            if sample <= *bound {
                self.counts[i].fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        self.overflow.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> HistogramSnapshot {
        HistogramSnapshot {
            buckets: self.buckets.clone(),
            counts: self.counts.iter().map(|c| c.load(Ordering::Relaxed)).collect(),
            overflow: self.overflow.load(Ordering::Relaxed),
            sum: self.sum.load(Ordering::Relaxed),
            count: self.count.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Debug)]
pub struct HistogramSnapshot {
    pub buckets: Vec<u64>,
    pub counts: Vec<u64>,
    pub overflow: u64,
    pub sum: u64,
    pub count: u64,
}

impl HistogramSnapshot {
    pub fn mean(&self) -> Option<f64> {
        if self.count == 0 { None }
        else { Some(self.sum as f64 / self.count as f64) }
    }
}

/// In-process metric registry. Multiple clones share state
/// via `Arc<RwLock>`. Metric names are free-form strings.
#[derive(Debug, Clone, Default)]
pub struct MetricsRegistry {
    inner: Arc<RwLock<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    counters: BTreeMap<String, Arc<Counter>>,
    histograms: BTreeMap<String, Arc<Histogram>>,
}

impl MetricsRegistry {
    pub fn new() -> Self { Self::default() }

    /// Get-or-create a counter by name. Multiple callers
    /// sharing the same name share the counter.
    pub fn counter(&self, name: &str) -> Arc<Counter> {
        {
            let r = self.inner.read();
            if let Some(c) = r.counters.get(name) { return c.clone(); }
        }
        let mut w = self.inner.write();
        w.counters.entry(name.into()).or_insert_with(|| Arc::new(Counter::default())).clone()
    }

    /// Get-or-create a histogram. First call wins on bucket
    /// layout; subsequent calls with different buckets reuse
    /// the first layout (operator diagnostic, not a sharp
    /// contract).
    pub fn histogram(&self, name: &str, buckets: Vec<u64>) -> Arc<Histogram> {
        {
            let r = self.inner.read();
            if let Some(h) = r.histograms.get(name) { return h.clone(); }
        }
        let mut w = self.inner.write();
        w.histograms.entry(name.into())
            .or_insert_with(|| Arc::new(Histogram::new(buckets)))
            .clone()
    }

    /// Snapshot of every counter value.
    pub fn counter_snapshot(&self) -> BTreeMap<String, u64> {
        self.inner.read().counters.iter()
            .map(|(k, v)| (k.clone(), v.get())).collect()
    }

    /// Snapshot of every histogram.
    pub fn histogram_snapshot(&self) -> BTreeMap<String, HistogramSnapshot> {
        self.inner.read().histograms.iter()
            .map(|(k, v)| (k.clone(), v.snapshot())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_increments_atomically() {
        let r = MetricsRegistry::new();
        let c = r.counter("ops.total");
        c.increment();
        c.increment();
        c.add(10);
        assert_eq!(c.get(), 12);
    }

    #[test]
    fn multiple_callers_share_the_same_counter() {
        let r = MetricsRegistry::new();
        let c1 = r.counter("shared");
        let c2 = r.counter("shared");
        c1.increment();
        c2.increment();
        assert_eq!(r.counter("shared").get(), 2);
    }

    #[test]
    fn histogram_buckets_count_correctly() {
        let r = MetricsRegistry::new();
        let h = r.histogram("latency.ms", vec![10, 100, 1000]);
        h.observe(5);   // bucket 0 (<=10)
        h.observe(50);  // bucket 1 (<=100)
        h.observe(500); // bucket 2 (<=1000)
        h.observe(5000); // overflow
        let s = h.snapshot();
        assert_eq!(s.counts, vec![1, 1, 1]);
        assert_eq!(s.overflow, 1);
        assert_eq!(s.count, 4);
        assert_eq!(s.sum, 5 + 50 + 500 + 5000);
        assert_eq!(s.mean(), Some((5 + 50 + 500 + 5000) as f64 / 4.0));
    }

    #[test]
    fn registry_snapshot_sorts_by_name() {
        let r = MetricsRegistry::new();
        r.counter("zeta").increment();
        r.counter("alpha").increment();
        let snap = r.counter_snapshot();
        let names: Vec<_> = snap.keys().collect();
        assert_eq!(names, vec!["alpha", "zeta"]);
    }

    #[test]
    fn empty_histogram_mean_is_none() {
        let r = MetricsRegistry::new();
        let h = r.histogram("empty", vec![10]);
        assert!(h.snapshot().mean().is_none());
    }
}
