//! ADR 0127 — in-memory non-unique secondary index helper.
//!
//! BRD-IDX-002 ("non-unique secondary indexes") is OPEN. This
//! module ships the correctness PRIMITIVE a future SQL
//! INSERT/UPDATE/DELETE path can call to maintain an
//! in-memory secondary index. It does NOT ship:
//!
//! - Disk-backed storage / WAL integration.
//! - Concurrent writers (callers hold an external lock).
//! - Catalog `CREATE INDEX` DDL wiring.
//! - Range-scan admission / resource accounting.
//! - Query-planner integration.
//! - Secondary-key *encoding*. The caller passes already-
//!   encoded byte keys, same contract as RowStore. This
//!   keeps the index agnostic to collation / schema shape.
//!
//! Those remain follow-up ADRs.

use std::collections::BTreeMap;
use std::ops::Bound;

/// A non-unique secondary index keyed by caller-encoded
/// `Vec<u8>` mapping to one or more primary-key byte blobs.
///
/// Byte keys preserve lexicographic ordering. Callers that
/// want numeric ordering must use big-endian / sign-bit
/// flipped encodings (same contract as the checkpoint
/// B+tree).
#[derive(Clone, Debug, Default)]
pub struct NonUniqueIndex {
    inner: BTreeMap<Vec<u8>, Vec<Vec<u8>>>,
}

impl NonUniqueIndex {
    pub fn new() -> Self { Self::default() }

    /// Insert `(secondary_key, primary_key)` edge. Idempotent
    /// per exact `(secondary_key, primary_key)` pair: a
    /// duplicate insert is a no-op.
    pub fn insert(&mut self, secondary_key: Vec<u8>, primary_key: Vec<u8>) {
        let entry = self.inner.entry(secondary_key).or_default();
        if !entry.contains(&primary_key) {
            entry.push(primary_key);
        }
    }

    /// Delete `(secondary_key, primary_key)` edge. Returns
    /// true if a matching edge existed. An empty bucket is
    /// cleaned up to preserve `distinct_keys()` accuracy.
    pub fn delete(&mut self, secondary_key: &[u8], primary_key: &[u8]) -> bool {
        let Some(entry) = self.inner.get_mut(secondary_key) else { return false; };
        let before = entry.len();
        entry.retain(|pk| pk.as_slice() != primary_key);
        let removed = entry.len() < before;
        if entry.is_empty() { self.inner.remove(secondary_key); }
        removed
    }

    /// Lookup every primary key mapped to an exact
    /// `secondary_key`. Returns an empty slice iff none.
    pub fn lookup(&self, secondary_key: &[u8]) -> &[Vec<u8>] {
        self.inner.get(secondary_key).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Range scan: yield `(&secondary_key, &primary_key)`
    /// pairs in sorted secondary-key order, bounded by
    /// `low` / `high`. Ordering within an equal
    /// secondary-key bucket is insertion order.
    pub fn range<'a>(
        &'a self,
        low: Bound<&'a [u8]>,
        high: Bound<&'a [u8]>,
    ) -> impl Iterator<Item = (&'a Vec<u8>, &'a Vec<u8>)> {
        let lo = match low {
            Bound::Included(v) => Bound::Included(v.to_vec()),
            Bound::Excluded(v) => Bound::Excluded(v.to_vec()),
            Bound::Unbounded   => Bound::Unbounded,
        };
        let hi = match high {
            Bound::Included(v) => Bound::Included(v.to_vec()),
            Bound::Excluded(v) => Bound::Excluded(v.to_vec()),
            Bound::Unbounded   => Bound::Unbounded,
        };
        self.inner.range((lo, hi))
            .flat_map(|(sk, pks)| pks.iter().map(move |pk| (sk, pk)))
    }

    /// Total number of `(secondary_key, primary_key)` edges.
    pub fn len(&self) -> usize {
        self.inner.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool { self.inner.is_empty() }

    /// Number of distinct secondary keys.
    pub fn distinct_keys(&self) -> usize { self.inner.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sk(s: &str) -> Vec<u8> { s.as_bytes().to_vec() }
    fn pk(i: i64) -> Vec<u8> { i.to_be_bytes().to_vec() }

    #[test]
    fn insert_and_lookup_single_edge() {
        let mut idx = NonUniqueIndex::new();
        idx.insert(sk("acme"), pk(1));
        assert_eq!(idx.lookup(&sk("acme")), &[pk(1)]);
        let empty: &[Vec<u8>] = &[];
        assert_eq!(idx.lookup(&sk("other")), empty);
    }

    #[test]
    fn insert_many_pks_under_same_secondary_key() {
        let mut idx = NonUniqueIndex::new();
        idx.insert(sk("acme"), pk(1));
        idx.insert(sk("acme"), pk(2));
        idx.insert(sk("acme"), pk(3));
        assert_eq!(idx.lookup(&sk("acme")).len(), 3);
        assert_eq!(idx.distinct_keys(), 1);
        assert_eq!(idx.len(), 3);
    }

    #[test]
    fn duplicate_insert_is_idempotent() {
        let mut idx = NonUniqueIndex::new();
        idx.insert(sk("acme"), pk(1));
        idx.insert(sk("acme"), pk(1));
        assert_eq!(idx.len(), 1);
    }

    #[test]
    fn delete_removes_only_matching_pk() {
        let mut idx = NonUniqueIndex::new();
        idx.insert(sk("acme"), pk(1));
        idx.insert(sk("acme"), pk(2));
        assert!(idx.delete(&sk("acme"), &pk(1)));
        assert_eq!(idx.lookup(&sk("acme")), &[pk(2)]);
    }

    #[test]
    fn delete_last_pk_cleans_up_bucket() {
        let mut idx = NonUniqueIndex::new();
        idx.insert(sk("acme"), pk(1));
        idx.delete(&sk("acme"), &pk(1));
        assert_eq!(idx.distinct_keys(), 0);
        assert!(idx.is_empty());
    }

    #[test]
    fn delete_nonexistent_returns_false() {
        let mut idx = NonUniqueIndex::new();
        idx.insert(sk("acme"), pk(1));
        assert!(!idx.delete(&sk("acme"), &pk(99)));
        assert!(!idx.delete(&sk("nope"), &pk(1)));
    }

    #[test]
    fn range_scan_is_sorted_by_secondary_key() {
        let mut idx = NonUniqueIndex::new();
        idx.insert(sk("zeta"), pk(1));
        idx.insert(sk("alpha"), pk(2));
        idx.insert(sk("mu"), pk(3));
        let keys: Vec<_> = idx.range(Bound::Unbounded, Bound::Unbounded)
            .map(|(sk, _)| sk.clone()).collect();
        assert_eq!(keys, vec![sk("alpha"), sk("mu"), sk("zeta")]);
    }

    #[test]
    fn range_scan_respects_bounds() {
        let mut idx = NonUniqueIndex::new();
        for s in ["a", "b", "c", "d", "e"] { idx.insert(sk(s), pk(0)); }
        let b = sk("b");
        let d = sk("d");
        let got: Vec<_> = idx.range(Bound::Included(&b), Bound::Excluded(&d))
            .map(|(sk, _)| sk.clone()).collect();
        assert_eq!(got, vec![sk("b"), sk("c")]);
    }

    #[test]
    fn range_scan_within_bucket_preserves_insertion_order() {
        let mut idx = NonUniqueIndex::new();
        idx.insert(sk("k"), pk(10));
        idx.insert(sk("k"), pk(20));
        idx.insert(sk("k"), pk(5));
        let pks: Vec<_> = idx.range(Bound::Unbounded, Bound::Unbounded)
            .map(|(_, pk)| pk.clone()).collect();
        assert_eq!(pks, vec![pk(10), pk(20), pk(5)]);
    }

    #[test]
    fn len_and_is_empty_track_total_edges() {
        let mut idx = NonUniqueIndex::new();
        assert!(idx.is_empty());
        idx.insert(sk("a"), pk(1));
        idx.insert(sk("a"), pk(2));
        idx.insert(sk("b"), pk(3));
        assert_eq!(idx.len(), 3);
        assert_eq!(idx.distinct_keys(), 2);
        idx.delete(&sk("a"), &pk(1));
        idx.delete(&sk("a"), &pk(2));
        idx.delete(&sk("b"), &pk(3));
        assert!(idx.is_empty());
    }

    #[test]
    fn empty_bounded_range_yields_nothing() {
        let mut idx = NonUniqueIndex::new();
        idx.insert(sk("b"), pk(1));
        let a = sk("a");
        let got: Vec<_> = idx.range(Bound::Included(&a), Bound::Excluded(&a)).collect();
        assert!(got.is_empty());
    }
}
