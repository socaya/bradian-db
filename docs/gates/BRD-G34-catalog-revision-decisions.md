# BRD-G34 — Durable catalog revision activation and cancellation

**Reference:** [ADR 0029](../adr/0029-catalog-revision-decisions.md)

**Status:** Catalog-only implementation and Linux regression tests passed.

**Evidence:** [Executed verification](../implementation/BRADIAN_VERIFICATION_2026_11_12.md).

G34 extends the G33 proposal journal with activation and cancellation. The receipt
contains the proposal's own WAL LSN and complete definition; stale receipts, even for
an identical later proposal, cannot decide it. Both decision paths validate under the
catalog mutex, sync one WAL frame, then publish. Replay uses the same event validator.
Rejected decisions leave the WAL position and authoritative metadata unchanged.

```rust
use bradian_relational::persistent_catalog::PersistentCatalog;
# fn example(catalog: &PersistentCatalog, id: bradian_relational::TableId,
# proposed: bradian_relational::Table) -> Result<(), Box<dyn std::error::Error>> {
catalog.propose_table_revision(id, proposed)?;
let receipt = catalog.pending_table_revision(id).unwrap();
catalog.activate_table_revision(receipt)?;
# Ok(())
# }
```

Activation preserves table identity, prior columns and constraints, appending nullable
columns only. Invalid names/types are rejected by authoritative catalog validators.
Cancellation keeps the current schema and releases the pending-proposal table-drop
fence. Foreign-key changes to a pending table are fenced. Original snapshots remain
immutable. Journal events retain the full original proposals and decisions.

## Coverage

Ten new catalog tests cover activation across checkpoint plus WAL and restart,
cancellation/activation backup restore, stale and mismatched receipts, successive
revisions without ID allocation, invalid identifiers/types, racing activation versus
cancellation, forged journal decisions, constraint fences, process exit without
Rust destructors, and twelve partial decision-frame truncation/retry cases. One new
historical SQL test proves independent catalog activation refuses mismatched schema
pins and leaves original typed historical rows intact.

## Boundary

This is a catalog metadata API, not SQL ALTER TABLE. Activating a catalog definition
independently of an existing typed row domain makes that root fail schema-pin checks.
G30 catalog horizon fences remain enforced. Do not use this to upgrade a historical
root; coordinated cross-WAL activation and mixed-schema SQL decoding are still needed.

No old WAL, row or page bytes are rewritten. New decisions extend the existing
version-1 JSON command vocabulary; older readers reject unknown events. Tests cover
process exit and synthetic partial writes, not real device power loss or disk-full
fault injection. Automatic CI remains manual-only.
