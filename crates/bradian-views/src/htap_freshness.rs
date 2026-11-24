//! ADR 0295 — LSN-based freshness annotation for HTAP reads.
//!
//! An HTAP reader deciding whether to serve from a
//! materialised view or fall back to a direct row scan
//! needs an explicit "view is current to LSN V; source is
//! at LSN S" signal. This type is a one-field-per-side
//! primitive calculated at read time; callers hold the
//! `view_lsn` on the maintained view and read `source_lsn`
//! from `MultiTableStore::last_lsn()` (or equivalent).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HtapFreshness {
    /// Last `CommitReceipt::lsn` this view absorbed.
    pub view_lsn: u64,
    /// Current source WAL LSN at read time.
    pub source_lsn: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HtapStaleness {
    Fresh,
    Behind { lag: u64 },
    /// view_lsn > source_lsn. Impossible in a single-writer
    /// shared-WAL domain; surfaced so a stale cache or a
    /// cross-replica mismatch is explicit rather than
    /// silently clamped.
    Ahead { excess: u64 },
}

impl HtapFreshness {
    pub fn new(view_lsn: u64, source_lsn: u64) -> Self {
        Self { view_lsn, source_lsn }
    }

    pub fn is_fresh(&self) -> bool {
        self.view_lsn == self.source_lsn
    }

    /// Non-negative lag in LSN units. Saturates at zero when
    /// the view is ahead of the source — use `classify()` to
    /// detect that case explicitly.
    pub fn lag_lsn(&self) -> u64 {
        self.source_lsn.saturating_sub(self.view_lsn)
    }

    pub fn classify(&self) -> HtapStaleness {
        use std::cmp::Ordering::*;
        match self.view_lsn.cmp(&self.source_lsn) {
            Equal   => HtapStaleness::Fresh,
            Less    => HtapStaleness::Behind { lag: self.source_lsn - self.view_lsn },
            Greater => HtapStaleness::Ahead  { excess: self.view_lsn - self.source_lsn },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_covers_the_three_ordering_cases() {
        assert_eq!(HtapFreshness::new(10, 10).classify(), HtapStaleness::Fresh);
        assert_eq!(HtapFreshness::new(10, 10).lag_lsn(), 0);
        assert!   (HtapFreshness::new(10, 10).is_fresh());

        assert_eq!(HtapFreshness::new(10, 15).classify(), HtapStaleness::Behind { lag: 5 });
        assert_eq!(HtapFreshness::new(10, 15).lag_lsn(), 5);
        assert!  (!HtapFreshness::new(10, 15).is_fresh());

        assert_eq!(HtapFreshness::new(15, 10).classify(), HtapStaleness::Ahead  { excess: 5 });
        assert_eq!(HtapFreshness::new(15, 10).lag_lsn(), 0,
            "lag saturates at zero when view is ahead; use classify() instead");
        assert!  (!HtapFreshness::new(15, 10).is_fresh());
    }
}
