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
}
