//! ADR 0132 — point-in-time-recovery restore-plan selector.
//!
//! BRD-DR-002 "auto-PITR" is PARTIAL. ADR 0102 shipped the
//! PITR map (base-backup + incremental-delta descriptors);
//! ADR 0121 shipped the incremental-backup delta writer.
//! The gap was the PLANNER: given a target LSN and an
//! inventory of available backups + deltas, choose which
//! base + which delta chain to apply.
//!
//! This module ships that planner as a pure function. It
//! does NOT execute the restore — the caller passes the
//! resulting `RestorePlan` to an executor.
//!
//! Honest scope:
//! - Pure function over caller-supplied descriptor slices.
//! - Chooses the LATEST base backup whose LSN is ≤ target.
//! - Walks deltas whose base LSN matches the chosen base
//!   and whose tip LSN is ≤ target, chaining by contiguous
//!   base_lsn → tip_lsn.
//! - Rejects: no base ≤ target, chain gap, target between
//!   two deltas without an intervening tip.
//!
//! Out of scope (follow-ups):
//! - Executor / file I/O — this is a planner only.
//! - Verification of backup/delta integrity — assumes the
//!   descriptors are already validated.
//! - Multi-tenant / per-shard planning.
//! - Delta compaction decisions.

use thiserror::Error;

/// Descriptor of a base backup in the inventory. Produced
/// by a scan of the backup directory (ADR 0015 receipts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseBackup {
    pub id: String,
    pub tip_lsn: u64,
}

/// Descriptor of an incremental delta (ADR 0121). Each
/// delta is anchored to a `base_lsn` and extends to
/// `tip_lsn`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeltaBackup {
    pub id: String,
    pub base_lsn: u64,
    pub tip_lsn: u64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PlannerError {
    #[error("no base backup with tip_lsn <= target_lsn ({target_lsn})")]
    NoBase { target_lsn: u64 },
    #[error("delta chain has a gap between lsn {expected} and {found}")]
    ChainGap { expected: u64, found: u64 },
    #[error("target_lsn {target_lsn} is beyond the latest reachable lsn {reachable}")]
    TargetBeyondReach { target_lsn: u64, reachable: u64 },
    #[error("delta {id:?} has base_lsn {base} > tip_lsn {tip}")]
    MalformedDelta { id: String, base: u64, tip: u64 },
}

pub type PlannerResult<T> = std::result::Result<T, PlannerError>;

/// Resulting plan. `base` is applied first, then each
/// delta in `deltas` order. Executor is responsible for
/// WAL-replay truncation to `target_lsn` after the final
/// delta is applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestorePlan {
    pub base: BaseBackup,
    pub deltas: Vec<DeltaBackup>,
    pub target_lsn: u64,
}

/// ADR 0132 — compute the restore plan for `target_lsn`.
pub fn plan_restore(
    target_lsn: u64,
    bases: &[BaseBackup],
    deltas: &[DeltaBackup],
) -> PlannerResult<RestorePlan> {
    // Validate every delta descriptor first.
    for d in deltas {
        if d.base_lsn > d.tip_lsn {
            return Err(PlannerError::MalformedDelta {
                id: d.id.clone(),
                base: d.base_lsn,
                tip: d.tip_lsn,
            });
        }
    }

    // Pick the LATEST base backup whose tip_lsn <= target.
    let base = bases.iter()
        .filter(|b| b.tip_lsn <= target_lsn)
        .max_by_key(|b| b.tip_lsn)
        .cloned()
        .ok_or(PlannerError::NoBase { target_lsn })?;

    // If the base already covers target, no deltas needed.
    if base.tip_lsn == target_lsn {
        return Ok(RestorePlan { base, deltas: Vec::new(), target_lsn });
    }

    // Walk the delta chain by contiguous base_lsn → tip_lsn,
    // starting from the base's tip_lsn, bounded by target.
    let mut cursor = base.tip_lsn;
    let mut chain: Vec<DeltaBackup> = Vec::new();
    let mut remaining: Vec<&DeltaBackup> = deltas.iter()
        .filter(|d| d.tip_lsn <= target_lsn)
        .collect();
    // Keep picking the delta whose base_lsn matches cursor.
    // If multiple match, pick the one with the largest
    // tip_lsn to advance faster.
    loop {
        let next = remaining.iter()
            .filter(|d| d.base_lsn == cursor)
            .max_by_key(|d| d.tip_lsn)
            .cloned();
        let Some(d) = next else { break; };
        cursor = d.tip_lsn;
        let d_owned = d.clone();
        chain.push(d_owned.clone());
        remaining.retain(|x| x.id != d_owned.id);
        if cursor == target_lsn { break; }
    }

    if cursor < target_lsn {
        return Err(PlannerError::TargetBeyondReach {
            target_lsn,
            reachable: cursor,
        });
    }

    Ok(RestorePlan { base, deltas: chain, target_lsn })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(id: &str, tip: u64) -> BaseBackup {
        BaseBackup { id: id.into(), tip_lsn: tip }
    }
    fn delta(id: &str, base: u64, tip: u64) -> DeltaBackup {
        DeltaBackup { id: id.into(), base_lsn: base, tip_lsn: tip }
    }

    #[test]
    fn base_exactly_covers_target_needs_no_deltas() {
        let bases = vec![base("b1", 100)];
        let plan = plan_restore(100, &bases, &[]).unwrap();
        assert_eq!(plan.base.id, "b1");
        assert!(plan.deltas.is_empty());
        assert_eq!(plan.target_lsn, 100);
    }

    #[test]
    fn selects_latest_base_not_exceeding_target() {
        let bases = vec![base("b1", 50), base("b2", 100), base("b3", 200)];
        let plan = plan_restore(150, &bases, &[delta("d", 100, 150)]).unwrap();
        assert_eq!(plan.base.id, "b2");
        assert_eq!(plan.deltas.len(), 1);
    }

    #[test]
    fn no_base_at_or_before_target_errors() {
        let bases = vec![base("b1", 200)];
        let err = plan_restore(100, &bases, &[]).unwrap_err();
        assert_eq!(err, PlannerError::NoBase { target_lsn: 100 });
    }

    #[test]
    fn chain_reaches_target_exactly() {
        let bases = vec![base("b1", 100)];
        let deltas = vec![
            delta("d1", 100, 150),
            delta("d2", 150, 200),
            delta("d3", 200, 300),
        ];
        let plan = plan_restore(200, &bases, &deltas).unwrap();
        assert_eq!(plan.deltas.iter().map(|d| &d.id[..])
                   .collect::<Vec<_>>(), vec!["d1", "d2"]);
    }

    #[test]
    fn chain_gap_errors_target_beyond_reach() {
        let bases = vec![base("b1", 100)];
        let deltas = vec![
            delta("d1", 100, 150),
            // GAP: nothing at 150
            delta("d3", 200, 300),
        ];
        let err = plan_restore(300, &bases, &deltas).unwrap_err();
        assert!(matches!(err, PlannerError::TargetBeyondReach { .. }));
    }

    #[test]
    fn deltas_above_target_are_ignored() {
        let bases = vec![base("b1", 100)];
        let deltas = vec![
            delta("d1", 100, 150),
            delta("d2", 150, 500),   // beyond target
        ];
        let plan = plan_restore(150, &bases, &deltas).unwrap();
        assert_eq!(plan.deltas.len(), 1);
        assert_eq!(plan.deltas[0].id, "d1");
    }

    #[test]
    fn when_multiple_deltas_match_cursor_picks_largest_tip() {
        let bases = vec![base("b1", 100)];
        let deltas = vec![
            delta("short", 100, 120),
            delta("long",  100, 200),
        ];
        let plan = plan_restore(200, &bases, &deltas).unwrap();
        // Should pick the long jump.
        assert_eq!(plan.deltas.len(), 1);
        assert_eq!(plan.deltas[0].id, "long");
    }

    #[test]
    fn malformed_delta_rejected_before_planning() {
        let bases = vec![base("b1", 100)];
        let deltas = vec![delta("bad", 200, 150)];
        let err = plan_restore(300, &bases, &deltas).unwrap_err();
        assert!(matches!(err, PlannerError::MalformedDelta { .. }));
    }

    #[test]
    fn target_matching_base_ignores_deltas_from_same_lsn() {
        let bases = vec![base("b1", 100)];
        let deltas = vec![delta("d1", 100, 150)];
        let plan = plan_restore(100, &bases, &deltas).unwrap();
        assert!(plan.deltas.is_empty());
    }

    #[test]
    fn target_beyond_chain_errors() {
        let bases = vec![base("b1", 100)];
        let deltas = vec![delta("d1", 100, 150)];
        let err = plan_restore(300, &bases, &deltas).unwrap_err();
        assert!(matches!(err,
            PlannerError::TargetBeyondReach { target_lsn: 300, reachable: 150 }));
    }

    #[test]
    fn empty_inventory_errors() {
        let err = plan_restore(100, &[], &[]).unwrap_err();
        assert_eq!(err, PlannerError::NoBase { target_lsn: 100 });
    }
}
