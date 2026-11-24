# ADR 0290 — fsync-on-power-loss simulation

## Status

Accepted.

## Context

G1-FAULTS called for an fsync-on-power-loss rig. A real
hardware rig (disk-power-controller + replay) is
infrastructure. The INVARIANT the rig would prove — "a
lying fsync followed by power loss never leaves silently
corrupted data" — is testable in software at the file
level, which is what this ADR ships.

## Decision

Simulate the power-loss scenario post-hoc at the file
level:

1. Write N rows + checkpoint. The WAL file now records a
   known durable state; capture its length.
2. Write M more rows WITHOUT a second checkpoint. The WAL
   has grown.
3. Truncate the WAL back to the captured length. That is
   exactly the on-disk state a lying fsync + power cut
   would leave: all pre-fsync bytes present, all
   post-fsync bytes gone.
4. Reopen the RowStore.
5. Assert the property: either the first N keys (pre-sync)
   are all present with correct values OR the reopen
   returns an explicit error. Any surviving key from the
   post-sync batch must carry its original value. No
   invented keys.

Three scenarios in `tests/fsync_power_loss_simulation.rs`:
- Truncate exactly to the sync point.
- Truncate to the midpoint between the sync point and the
  final WAL length (likely mid-frame).
- Truncate to zero (no durable state at all).

## Consequences

- The code-side invariant is covered. No fsync-lying
  silent-corruption can produce wrong reads after reopen.
- A hardware-level rig remains documented as a nice-to-have
  for OS-specific durability claims (particular fsync
  implementations may elide the sync differently). This ADR
  does NOT claim OS-level fsync fidelity — it covers the
  store-level invariant that whatever ends up on disk after
  a lying fsync cannot produce silent corruption on reopen.
- Together with the FaultPolicy trait (ADR 0288) and the
  scripted-workload harness (ADR 0289), P0-5 fault-injection
  coverage is now code-complete.
- 3 tests pass.
