//! Per-column and per-page zone maps — min/max/null_count/row_count.
//!
//! Used by the planner for partition/segment/page pruning (STO-007, OPT-001).

use serde::{Deserialize, Serialize};

/// A zone map is a set of summary statistics attached to a page or segment column slice.
///
/// `min`/`max` are stored as the serialised byte form of the logical value. Comparisons are done
/// lexicographically for strings and numerically for integers/floats via dedicated typed
/// comparators that live in the planner. We intentionally do not model the typed range here
/// because storage should stay type-agnostic over its serialised bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoneMap {
    pub row_count: u32,
    pub null_count: u32,
    pub min: Option<Vec<u8>>,
    pub max: Option<Vec<u8>>,
}

impl ZoneMap {
    pub fn is_all_null(&self) -> bool {
        self.row_count > 0 && self.null_count == self.row_count
    }

    pub fn is_empty(&self) -> bool {
        self.row_count == 0
    }

    /// ADR 0081 §3.3 — identity element for merging. Starts with
    /// zero rows / zero nulls / no min/max; merge with any other
    /// returns the other unchanged (useful as a fold accumulator).
    pub fn empty() -> Self {
        Self::default()
    }

    /// ADR 0081 §3.3 — merge two page-level zone maps into a
    /// column-level zone map. Row/null counts add; min is the
    /// lexicographic lesser; max is the lexicographic greater.
    /// Preserves `Option<Vec<u8>>` semantics: an absent bound
    /// (all-null page) yields the other page's bound unchanged.
    pub fn merge(&self, other: &ZoneMap) -> ZoneMap {
        ZoneMap {
            row_count: self.row_count.saturating_add(other.row_count),
            null_count: self.null_count.saturating_add(other.null_count),
            min: match (&self.min, &other.min) {
                (None, None) => None,
                (Some(a), None) => Some(a.clone()),
                (None, Some(b)) => Some(b.clone()),
                (Some(a), Some(b)) => Some(if a <= b { a.clone() } else { b.clone() }),
            },
            max: match (&self.max, &other.max) {
                (None, None) => None,
                (Some(a), None) => Some(a.clone()),
                (None, Some(b)) => Some(b.clone()),
                (Some(a), Some(b)) => Some(if a >= b { a.clone() } else { b.clone() }),
            },
        }
    }
}
