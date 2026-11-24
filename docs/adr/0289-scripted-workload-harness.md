# ADR 0289 — Scripted-workload fault harness

## Status

Accepted.

## Context

ADRs 0252–0255 and 0286/0287 covered fault sweeps at
specific offsets on fixed workloads. ADR 0289 adds a
scripted-workload harness: deterministic (seeded) random
op sequences × random fault point, so any new regression
is reproducible from its seed.

## Decision

New `tests/fault_scripted_harness.rs`:
- Tiny xorshift64 `Rng` seeded from a parameter.
- `generate_workload(seed, len)` emits a mix of
  `Put / Overwrite / Delete / Checkpoint` ops.
- `drive(dir, ops)` applies the workload against a
  `RowStore` and tracks **every ACKed version** of each key
  (not just the last) since a truncation can roll back to
  an earlier version.
- `assert_reopen_clean(dir, acked)` runs the standard
  subset-of-ACKed property: for every key the reopened
  store exposes, the value must be one of the historical
  ACKed versions for that key. No invented keys.
- Three scenarios parameterised by seed:
  - Byte flip at a random offset in a random file (5 seeds).
  - Truncation at a random point in a random file (5 seeds).
  - Clean reopen control.

## Consequences

- P0-5 harness is now scripted-and-seeded. A future
  regression surfaces with a reproducible seed.
- No injection at write-time yet — the FaultPolicy trait
  (ADR 0288) still awaits a RowStore retrofit to actually
  consult it per `put` / `sync`. The scripted harness
  applies faults post-hoc, which already covers the "did
  the on-disk layout tolerate corruption" invariant.
- 3 tests across ~11 seeded scenarios.
