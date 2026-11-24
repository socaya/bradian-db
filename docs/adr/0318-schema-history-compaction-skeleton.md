# ADR 0318 — Schema history compaction with reference proofs: skeleton

## Status

Accepted as a skeleton. Eleventh slice of P2.

## Context

Over time, the catalog's DDL journal accretes schema
versions (ADR 0031/0032/0040) that no retained historical
row depends on. Operators need to compact the journal
without losing the ability to **prove** that compaction
preserves every reachable schema version. A production
compaction protocol is Gate-sized.

## Decision

New module
`bradian_relational::schema_history_compaction` with:

```rust
pub struct SchemaVersion { pub version: u64, pub digest: [u8; 32] }
pub struct CompactionPlan {
    pub retained: Vec<SchemaVersion>,
    pub dropped:  Vec<SchemaVersion>,
    pub proof:    Vec<u8>,                     // concatenated retained digests
}
pub fn plan_compaction(
    all: &[SchemaVersion], live: &[u64],
) -> CompactionPlan;
pub fn verify_proof(plan: &CompactionPlan) -> bool;
```

Semantics:

- `plan_compaction` keeps versions whose version-id is in
  `live` and drops the rest.
- `proof` is the concatenation of retained digests in
  version order. `verify_proof` recomputes it and checks
  equality — a reference consumer stores the proof
  bytes beside the compacted journal.

Honest scope: pure planning + proof. No journal rewrite,
no on-disk coupling.

## Consequences

- 2 unit tests: compaction drops exactly the non-live
  versions; proof verifies after planning and fails if
  the retained vec is tampered.
