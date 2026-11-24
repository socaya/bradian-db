# ADR 0286 + 0287 — Fault injection on BRPG snapshot + manifest files

## Status

Accepted.

## Context

ADRs 0252–0255 covered WAL / user-journal / DDL-journal
fault sweeps. BRPG checkpoint files (`snapshot-N.brp`) and
manifest files (`manifest*`) had no fault-injection
coverage: a flipped byte in a snapshot page or a corrupted
manifest pointer could silently feed wrong data to readers.

## Decision

New `tests/fault_injection_checkpoint.rs`:
- `seeded_and_checkpointed()` creates a RowStore, writes 10
  rows, calls `checkpoint()`, closes, returns the list of
  `snapshot-*.brp` + `manifest*` file paths.
- `assert_recovery_is_clean(dir)` runs the standard property
  check: reopen either errors cleanly or exposes a
  (k,v) subset of what was committed.
- 4 scenarios:
  - Byte flip in each snapshot file (middle byte).
  - Truncate each snapshot file to 2/3 length.
  - Byte flip in each manifest file.
  - Zero-length snapshot file (pathological).
- Error allowlist expanded for this layer:
  `StoreError::{Corruption, Io, InvalidRow, Page, Manifest}`.

## Consequences

- P0-5 fault-injection coverage now spans WAL + checkpoint +
  manifest. Every durable file type RowStore writes has a
  property test.
- Build variations that skip emitting a manifest (some
  checkpoint paths) cause the manifest test to skip rather
  than fail — honest about the on-disk shape.
- 4 tests close ADRs 0286 (snapshot sweep) and 0287 (manifest
  sweep) together.
