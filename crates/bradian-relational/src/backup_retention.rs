//! ADR 0138 — backup retention policy planner.
//!
//! BRD-DR-003 "backup lifecycle / retention" is OPEN.
//! Companion to:
//!   - ADR 0015 base-backup receipts
//!   - ADR 0121 incremental-backup delta writer
//!   - ADR 0132 PITR restore-plan selector
//!
//! A retention plan is a PURE FUNCTION over the caller's
//! current inventory + a `RetentionPolicy`. The caller
//! enforces the plan on the filesystem; this module never
//! touches disk.
//!
//! Honest scope:
//! - Three policy knobs:
//!   - `min_keep_bases: u32` — always keep the latest N
//!     bases (floor; prevents accidentally pruning every
//!     base).
//!   - `min_recoverable_lsn: Option<u64>` — refuse to
//!     prune anything that would make LSN `x` unreachable
//!     via the ADR 0132 planner.
//!   - `max_age_days: Option<u32>` — bases older than this
//!     are eligible for pruning (deltas pinned to a kept
//!     base are also kept).
//! - Returns a `RetentionPlan { keep, prune }` partition.
//! - Orphan delta cleanup: deltas whose base is pruned
//!   AND whose tip is not reachable from any kept base are
//!   pruned too.
//!
//! Out of scope (follow-ups):
//! - Filesystem execution — caller deletes.
//! - Encrypted-backup rotation — this is retention, not
//!   key rotation.
//! - Cross-tenant / per-database retention.
//! - Automatic-run scheduling.

use std::collections::BTreeSet;

use crate::pitr_planner::{BaseBackup, DeltaBackup};

/// Retention knobs. `min_keep_bases` has a hard floor of 1
/// (never leave the system with zero bases).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetentionPolicy {
    pub min_keep_bases: u32,
    pub min_recoverable_lsn: Option<u64>,
    pub max_age_days: Option<u32>,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self { min_keep_bases: 2, min_recoverable_lsn: None, max_age_days: None }
    }
}

/// Descriptor extension: age in days + unique id. A real
/// inventory walk fills these from the on-disk receipts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgedBase {
    pub base: BaseBackup,
    pub age_days: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgedDelta {
    pub delta: DeltaBackup,
    pub age_days: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RetentionPlan {
    pub keep_bases: Vec<String>,
    pub prune_bases: Vec<String>,
    pub keep_deltas: Vec<String>,
    pub prune_deltas: Vec<String>,
}

/// ADR 0138 — compute the retention plan.
///
/// Rules (in order):
/// 1. The latest `min_keep_bases.max(1)` bases are kept.
/// 2. If `min_recoverable_lsn` is set, every base + delta
///    needed to reach that LSN is kept.
/// 3. Otherwise, bases older than `max_age_days` are
///    eligible for pruning.
/// 4. A delta is kept iff its base is kept AND its
///    tip_lsn is reachable from the kept chain.
pub fn plan_retention(
    bases: &[AgedBase],
    deltas: &[AgedDelta],
    policy: &RetentionPolicy,
) -> RetentionPlan {
    let floor = policy.min_keep_bases.max(1) as usize;

    // Sort bases by tip_lsn desc so "latest N" picks the
    // highest LSNs (stable regardless of input order).
    let mut by_lsn: Vec<&AgedBase> = bases.iter().collect();
    by_lsn.sort_by_key(|a| std::cmp::Reverse(a.base.tip_lsn));

    // Start by keeping the latest `floor` bases.
    let mut keep: BTreeSet<String> = by_lsn.iter()
        .take(floor)
        .map(|b| b.base.id.clone())
        .collect();

    // Rule 3: prune bases older than max_age_days UNLESS
    // in the floor set.
    if let Some(max_age) = policy.max_age_days {
        for b in bases {
            if b.age_days > max_age && !keep.contains(&b.base.id) {
                // leave out
            } else {
                keep.insert(b.base.id.clone());
            }
        }
    } else {
        // No age bound: keep every base NOT in the floor
        // set as well. (Retention only kicks in when either
        // floor or max_age_days narrows the set.)
        for b in bases {
            keep.insert(b.base.id.clone());
        }
    }

    // Rule 2: coverage pin. If we need to reach a specific
    // LSN, make sure the latest base <= that LSN is kept.
    if let Some(target) = policy.min_recoverable_lsn {
        if let Some(pin) = bases.iter()
            .filter(|b| b.base.tip_lsn <= target)
            .max_by_key(|b| b.base.tip_lsn)
        {
            keep.insert(pin.base.id.clone());
        }
    }

    let (keep_bases, prune_bases): (Vec<_>, Vec<_>) = bases.iter()
        .map(|b| b.base.id.clone())
        .partition(|id| keep.contains(id));

    // Kept-base tip LSNs; used to decide delta chainability.
    let kept_lsns: BTreeSet<u64> = bases.iter()
        .filter(|b| keep.contains(&b.base.id))
        .map(|b| b.base.tip_lsn)
        .collect();

    // A delta is kept iff:
    //   - its base_lsn equals a kept base's tip_lsn OR
    //     a kept delta's tip_lsn (chain extension), AND
    //   - if min_recoverable_lsn is set, its tip_lsn <=
    //     target (deltas beyond the pin aren't required).
    //
    // Simpler sufficient rule we use here: keep every delta
    // whose base_lsn is reachable in the kept tip set, and
    // iterate until the kept set is stable. Orphan deltas
    // (base LSN not in kept set) are pruned.
    let mut kept_tip_lsns = kept_lsns.clone();
    let mut keep_deltas = Vec::new();
    let mut prune_deltas = Vec::new();
    let mut remaining: Vec<&AgedDelta> = deltas.iter().collect();
    let target = policy.min_recoverable_lsn;
    loop {
        let before = keep_deltas.len();
        remaining.retain(|d| {
            let chainable = kept_tip_lsns.contains(&d.delta.base_lsn);
            let within_target = target.map(|t| d.delta.tip_lsn <= t).unwrap_or(true);
            if chainable && within_target {
                kept_tip_lsns.insert(d.delta.tip_lsn);
                keep_deltas.push(d.delta.id.clone());
                false  // consumed
            } else {
                true
            }
        });
        if keep_deltas.len() == before { break; }
    }
    for d in remaining {
        prune_deltas.push(d.delta.id.clone());
    }

    RetentionPlan { keep_bases, prune_bases, keep_deltas, prune_deltas }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(id: &str, lsn: u64, age: u32) -> AgedBase {
        AgedBase {
            base: BaseBackup { id: id.into(), tip_lsn: lsn },
            age_days: age,
        }
    }
    fn delta(id: &str, base_lsn: u64, tip_lsn: u64, age: u32) -> AgedDelta {
        AgedDelta {
            delta: DeltaBackup { id: id.into(), base_lsn, tip_lsn },
            age_days: age,
        }
    }

    #[test]
    fn min_keep_bases_preserves_latest_n() {
        let bases = vec![
            base("b1", 100, 30),
            base("b2", 200, 20),
            base("b3", 300, 10),
            base("b4", 400, 5),
        ];
        let policy = RetentionPolicy {
            min_keep_bases: 2, min_recoverable_lsn: None,
            max_age_days: Some(7),  // all but b4 older than 7
        };
        let plan = plan_retention(&bases, &[], &policy);
        // Latest 2 (b3, b4) always kept; b1, b2 pruned (older than 7).
        assert!(plan.keep_bases.contains(&"b3".to_string()));
        assert!(plan.keep_bases.contains(&"b4".to_string()));
        assert!(plan.prune_bases.contains(&"b1".to_string()));
        assert!(plan.prune_bases.contains(&"b2".to_string()));
    }

    #[test]
    fn floor_of_one_enforced_even_if_zero_requested() {
        let bases = vec![base("b1", 100, 1000)];
        let policy = RetentionPolicy {
            min_keep_bases: 0, min_recoverable_lsn: None,
            max_age_days: Some(10),
        };
        let plan = plan_retention(&bases, &[], &policy);
        assert_eq!(plan.keep_bases, vec!["b1".to_string()]);
        assert!(plan.prune_bases.is_empty());
    }

    #[test]
    fn min_recoverable_lsn_pins_covering_base() {
        let bases = vec![
            base("b1", 100, 100),
            base("b2", 200, 90),
            base("b3", 300, 80),
            base("b4", 400, 1),
        ];
        let policy = RetentionPolicy {
            min_keep_bases: 1,
            min_recoverable_lsn: Some(150), // need base <= 150
            max_age_days: Some(10),
        };
        let plan = plan_retention(&bases, &[], &policy);
        // b1 pins coverage of LSN 150.
        assert!(plan.keep_bases.contains(&"b1".to_string()));
        // b4 kept by floor.
        assert!(plan.keep_bases.contains(&"b4".to_string()));
    }

    #[test]
    fn orphan_deltas_pruned() {
        let bases = vec![base("b1", 100, 1), base("b2", 500, 1)];
        let deltas = vec![
            delta("d1", 100, 150, 1),
            delta("d2", 150, 200, 1),
            delta("orphan", 999, 1100, 1),
        ];
        let policy = RetentionPolicy::default();
        let plan = plan_retention(&bases, &deltas, &policy);
        assert!(plan.keep_deltas.contains(&"d1".to_string()));
        assert!(plan.keep_deltas.contains(&"d2".to_string()));
        assert_eq!(plan.prune_deltas, vec!["orphan".to_string()]);
    }

    #[test]
    fn deltas_whose_base_is_pruned_are_pruned() {
        let bases = vec![
            base("old", 100, 100),
            base("new", 500, 1),
        ];
        let deltas = vec![delta("d_old", 100, 150, 100)];
        let policy = RetentionPolicy {
            min_keep_bases: 1,
            min_recoverable_lsn: None,
            max_age_days: Some(10),
        };
        let plan = plan_retention(&bases, &deltas, &policy);
        assert!(plan.prune_bases.contains(&"old".to_string()));
        assert!(plan.prune_deltas.contains(&"d_old".to_string()));
    }

    #[test]
    fn deltas_beyond_target_are_pruned_when_pin_set() {
        let bases = vec![base("b1", 100, 1)];
        let deltas = vec![
            delta("d1", 100, 150, 1),
            delta("d2", 150, 300, 1),  // beyond target
        ];
        let policy = RetentionPolicy {
            min_keep_bases: 1,
            min_recoverable_lsn: Some(150),
            max_age_days: None,
        };
        let plan = plan_retention(&bases, &deltas, &policy);
        assert!(plan.keep_deltas.contains(&"d1".to_string()));
        assert!(plan.prune_deltas.contains(&"d2".to_string()));
    }

    #[test]
    fn default_policy_keeps_everything() {
        let bases = vec![base("b1", 100, 0)];
        let deltas = vec![delta("d1", 100, 150, 0)];
        let plan = plan_retention(&bases, &deltas, &RetentionPolicy::default());
        assert_eq!(plan.keep_bases, vec!["b1".to_string()]);
        assert_eq!(plan.keep_deltas, vec!["d1".to_string()]);
        assert!(plan.prune_bases.is_empty());
        assert!(plan.prune_deltas.is_empty());
    }

    #[test]
    fn empty_inventory_yields_empty_plan() {
        let plan = plan_retention(&[], &[], &RetentionPolicy::default());
        assert_eq!(plan, RetentionPlan::default());
    }

    #[test]
    fn chain_with_gap_prunes_everything_past_the_gap() {
        let bases = vec![base("b1", 100, 1)];
        let deltas = vec![
            delta("d1", 100, 150, 1),
            // gap at 150
            delta("d3", 200, 300, 1),
        ];
        let plan = plan_retention(&bases, &deltas, &RetentionPolicy::default());
        assert!(plan.keep_deltas.contains(&"d1".to_string()));
        assert!(plan.prune_deltas.contains(&"d3".to_string()));
    }
}
