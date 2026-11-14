# Master Gate 1 — Deterministic fault recovery verification

14 November 2026. Baseline: `304d7836`.
Stable task G1-FAULTS in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**785 Rust tests passed, 0 failed, 0 ignored**, including **510 relational
tests**. Workspace compilation and the fault-recovery tests passed. Clippy
passed on `bradian-relational --all-targets`. CI remains manual-only. No
dependency, lockfile, persistent format, SQL grammar or default isolation
changed.

Environment: Darwin 25.2.0 x86_64, Rust/Cargo stable.
The checkout was restored from published `main` before implementation.

| Executed check | Result |
|---|---|
| `cargo check --workspace` | Passed |
| `cargo test --workspace` | 785 passed, 0 failed, 0 ignored |
| `cargo test -p bradian-relational --test fault_recovery` | 6 passed |
| `cargo clippy -p bradian-relational --all-targets -- -D warnings` | Passed |

Studio typecheck/build and `scripts/verify_roadmap.py` were not executed on
this increment — the earlier verification report records those passes.

## Implemented and exercised

Six new tests in `tests/fault_recovery.rs` cover crash outcomes distinct from
the existing CRC/truncation/manifest-regression matrix:

1. Zero-padded WAL extension beyond the last frame — the WAL magic check
   refuses a NUL header as `StoreError::Corruption("bad WAL magic…")` rather
   than silently trimming.
2. Complete header followed by an all-zero body — recovery fails closed with
   `StoreError::Corruption("WAL CRC mismatch…")` (or a sequence error) rather
   than accepting the body as "incomplete tail".
3. `snapshot-<lsn>.tmp` and `index-<lsn>.tmp` orphans at a higher LSN than the
   committed generation — recovery uses the manifest's committed generation
   and leaves the `.tmp` artifacts untouched.
4. Multiple aborted checkpoint generations with `.tmp` artifacts at distinct
   LSNs — recovery still uses the committed snapshot and a subsequent
   checkpoint succeeds without touching the orphans.
5. Incidental files in the store directory (`.DS_Store`, `README`,
   `snapshot-backup.bak`) — neither the manifest reopen path nor the
   pre-manifest discovery path breaks.
6. Pending manifest leftover whose bytes match the current marker — not
   promoted to authoritative without going through `publish`; a subsequent
   checkpoint publishes a new marker normally.

No production code is modified in this increment; the tests exercise the real
recovery path on real files.

## Remaining scope

Real `ENOSPC`/`EIO` propagation at every call site, `fsync` failure on power
loss, torn-write behavior on physical devices, Windows and macOS device-level
qualification, remote and encrypted filesystems, long-running operational
recovery rehearsals and multi-process shared-WAL/history-SQL fault matrices
all remain open. See [ADR 0052](../adr/0052-deterministic-fault-recovery.md)
and the [fixed roadmap](BRADIAN_MASTER_ROADMAP.md).
