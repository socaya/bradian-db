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

    /// ADR 0125 — render counters + histograms as the
    /// Prometheus text exposition format (v0.0.4).
    ///
    /// Counters emit a single `<name> <value>` line with a
    /// `# TYPE counter` header. Histograms emit one line per
    /// cumulative bucket (`<name>_bucket{le="..."}`) plus
    /// `_bucket{le="+Inf"}`, `_sum` and `_count` with a
    /// `# TYPE histogram` header.
    ///
    /// Metric names are sanitized to `[a-zA-Z0-9_]`; dots
    /// in the Bradian naming convention map to `_` so
    /// `ops.total` becomes `ops_total`.
    pub fn render_prometheus(&self) -> String {
        let mut out = String::new();
        let inner = self.inner.read();
        for (name, counter) in &inner.counters {
            let n = sanitize_metric_name(name);
            out.push_str(&format!("# TYPE {n} counter\n{n} {}\n", counter.get()));
        }
        for (name, hist) in &inner.histograms {
            let n = sanitize_metric_name(name);
            let s = hist.snapshot();
            out.push_str(&format!("# TYPE {n} histogram\n"));
            let mut cumulative: u64 = 0;
            for (i, bound) in s.buckets.iter().enumerate() {
                cumulative = cumulative.saturating_add(s.counts[i]);
                out.push_str(&format!("{n}_bucket{{le=\"{bound}\"}} {cumulative}\n"));
            }
            let total = cumulative.saturating_add(s.overflow);
            out.push_str(&format!("{n}_bucket{{le=\"+Inf\"}} {total}\n"));
            out.push_str(&format!("{n}_sum {}\n", s.sum));
            out.push_str(&format!("{n}_count {}\n", s.count));
        }
        out
    }
}

/// Prometheus text format restricts metric names to
/// `[a-zA-Z_:][a-zA-Z0-9_:]*`. Map any other byte to `_`.
fn sanitize_metric_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for (i, ch) in raw.chars().enumerate() {
        let ok = if i == 0 {
            ch.is_ascii_alphabetic() || ch == '_' || ch == ':'
        } else {
            ch.is_ascii_alphanumeric() || ch == '_' || ch == ':'
        };
        out.push(if ok { ch } else { '_' });
    }
    if out.is_empty() { "_unnamed".into() } else { out }
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

    #[test]
    fn render_prometheus_emits_counter_lines() {
        let r = MetricsRegistry::new();
        r.counter("ops.total").add(7);
        let out = r.render_prometheus();
        assert!(out.contains("# TYPE ops_total counter\n"));
        assert!(out.contains("\nops_total 7\n"));
    }

    #[test]
    fn render_prometheus_emits_cumulative_histogram_buckets() {
        let r = MetricsRegistry::new();
        let h = r.histogram("latency.ms", vec![10, 100, 1000]);
        h.observe(5);   // bucket 0
        h.observe(50);  // bucket 1
        h.observe(500); // bucket 2
        h.observe(5000); // overflow
        let out = r.render_prometheus();
        assert!(out.contains("# TYPE latency_ms histogram\n"));
        assert!(out.contains("latency_ms_bucket{le=\"10\"} 1\n"));
        assert!(out.contains("latency_ms_bucket{le=\"100\"} 2\n"));
        assert!(out.contains("latency_ms_bucket{le=\"1000\"} 3\n"));
        assert!(out.contains("latency_ms_bucket{le=\"+Inf\"} 4\n"));
        assert!(out.contains("latency_ms_sum 5555\n"));
        assert!(out.contains("latency_ms_count 4\n"));
    }

    #[test]
    fn render_prometheus_sanitizes_metric_names() {
        let r = MetricsRegistry::new();
        r.counter("foo-bar/baz").increment();
        let out = r.render_prometheus();
        assert!(out.contains("foo_bar_baz 1"), "got {out}");
    }

    #[test]
    fn render_prometheus_empty_registry_returns_empty_string() {
        let r = MetricsRegistry::new();
        assert!(r.render_prometheus().is_empty());
    }

    #[test]
    fn render_prometheus_is_deterministic_in_sort_order() {
        let r = MetricsRegistry::new();
        r.counter("zeta").increment();
        r.counter("alpha").increment();
        let out = r.render_prometheus();
        // alpha appears before zeta (BTreeMap iteration)
        let a = out.find("alpha").unwrap();
        let z = out.find("zeta").unwrap();
        assert!(a < z);
    }
}
