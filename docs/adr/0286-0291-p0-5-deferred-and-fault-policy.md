# ADR 0286 + 0287 + 0288 + 0289 + 0290 + 0291 — P0-5 close-out

## Status

Accepted.

ADR 0288 ships as code; the rest are scope declarations for
the Platform Gaps Plan.

## Context

This autonomous run shipped 4 P0-5 slices earlier (ADRs
0252–0255: WAL fault primitive + user/DDL journal fault tests
+ RowStore sweep + user/DDL journal sweep). Six items
remained open.

## Decision

### Shipped — ADR 0288 FaultPolicy trait

- New `fault_injection::FaultPolicy` trait with
  `on_write` / `on_sync` hooks (both default to `Ok`).
- Reference implementations:
  - `NeverFail` — always Ok (sanity baseline).
  - `FailAfterNWrites(n)` — the Nth write returns an
    `io::Error`, subsequent calls succeed; one-shot.
  - `DropOnNthSync(n)` — the Nth sync returns Ok but sets a
    "fired" flag so callers can treat the store as not
    durably synced.
- All policies `Send + Sync` for concurrent test harnesses.

### Deferred with design notes

- **ADR 0286 — BRPG checkpoint sweep.** Extend ADR 0254's
  property sweep to `snapshot-N.brp` files. Trigger is
  harder — the test must drive the engine through at least
  one checkpoint before applying the fault. Design: seed
  N rows, call `TableStore::checkpoint`, then sweep.
- **ADR 0287 — Manifest sweep.** Same pattern as BRPG —
  `manifest-*` files. Trigger: force a manifest write
  via the lifecycle journal.
- **ADR 0289 — Scripted-workload property harness.**
  Random workload driver that interleaves `put` / `delete`
  / `scan` operations with fault points chosen by a
  policy. Builds on ADR 0288; needs a `RowStore` retrofit
  so writes consult the `FaultPolicy` — currently it's a
  test-only primitive without the storage-layer wiring.
- **ADR 0290 — fsync-on-power-loss rig.** External
  hardware or VM harness. Infrastructure out of scope for
  autonomous completion.
- **ADR 0291 — Windows installation + lock lifecycle.**
  Requires a Windows runner. macOS closed per
  [verification report]
  (../implementation/BRADIAN_GATE1_FAULT_RECOVERY_MACOS_VERIFICATION_2026_11_18.md).

## Consequences

- P0-5 ships 5 of 11 planned slices (ADRs 0252–0255 + 0288).
  6 remain open.
- `FaultPolicy` is reachable by test code now; wiring into
  `RowStore::put` / `RowStore::sync` is the next slice.
- 5 cases in `tests/fault_policy.rs` cover: NeverFail is
  no-op; FailAfterNWrites counts and fires once;
  DropOnNthSync marks fired without erroring; Send+Sync;
  concurrent fire fires exactly once.
