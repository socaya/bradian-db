//! ADR 0318 — Schema history compaction with reference
//! proofs **skeleton**.
//!
//! Pure planning + proof. No journal rewrite, no on-disk
//! coupling.

use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SchemaVersion {
    pub version: u64,
    pub digest:  [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactionPlan {
    pub retained: Vec<SchemaVersion>,
    pub dropped:  Vec<SchemaVersion>,
    pub proof:    Vec<u8>,
}

pub fn plan_compaction(all: &[SchemaVersion], live: &[u64]) -> CompactionPlan {
    let live_set: HashSet<u64> = live.iter().copied().collect();
    let mut retained = Vec::new();
    let mut dropped  = Vec::new();
    for v in all {
        if live_set.contains(&v.version) { retained.push(*v); } else { dropped.push(*v); }
    }
    retained.sort_by_key(|v| v.version);
    dropped.sort_by_key (|v| v.version);
    let mut proof = Vec::with_capacity(retained.len() * 32);
    for v in &retained { proof.extend_from_slice(&v.digest); }
    CompactionPlan { retained, dropped, proof }
}

pub fn verify_proof(plan: &CompactionPlan) -> bool {
    let mut recomputed = Vec::with_capacity(plan.retained.len() * 32);
    for v in &plan.retained { recomputed.extend_from_slice(&v.digest); }
    recomputed == plan.proof
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(v: u64, fill: u8) -> SchemaVersion {
        SchemaVersion { version: v, digest: [fill; 32] }
    }

    #[test]
    fn compaction_drops_exactly_the_non_live_versions() {
        let all = vec![version(1, 0xa), version(2, 0xb), version(3, 0xc), version(4, 0xd)];
        let plan = plan_compaction(&all, &[2, 4]);
        assert_eq!(plan.retained, vec![version(2, 0xb), version(4, 0xd)]);
        assert_eq!(plan.dropped,  vec![version(1, 0xa), version(3, 0xc)]);
        assert!(verify_proof(&plan));
    }

    #[test]
    fn tampering_with_retained_vec_fails_proof_verification() {
        let all = vec![version(1, 0xaa), version(2, 0xbb)];
        let mut plan = plan_compaction(&all, &[1, 2]);
        plan.retained[0].digest[0] ^= 0xff;
        assert!(!verify_proof(&plan));
    }
}
