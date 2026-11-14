# Master Gate 1 — Fault recovery bounds verification

14 November 2026. Baseline: `99607b4`.
Stable task G1-FAULTS in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**863 Rust tests passed, 0 failed, 0 ignored.** Clippy clean on
`bradian-relational --all-targets`. No persistent format change; the
engine behaviour is unchanged. Three new deterministic tests exercise
previously-unproven corners of the WAL length-bound check and the
Unix fd-lock lifecycle.

| Executed check | Result |
|---|---|
| `cargo test --workspace` | 863 passed, 0 failed |
| `cargo test -p bradian-relational --test fault_recovery` | 12 passed (+3 new) |
| `cargo clippy -p bradian-relational --all-targets -- -D warnings` | Passed |

## Finding and action

The gaps analysis §9 flagged "fault-recovery matrix extension" as a
code-only follow-up, with three concrete outcomes to pin down:

1. **WAL frame with declared length 0.** The engine's replay loop
   requires `13 <= len <= MAX_WAL_BODY` for an accepted frame. A
   length of 0 must refuse as `Corruption("invalid WAL frame length
   ...")` rather than being accepted as an "empty body" or trimmed as
   incomplete.
2. **WAL frame with declared length above `MAX_WAL_BODY` (1 MiB).**
   A malicious or corrupted header must not be able to force a
   gigabyte-scale body allocation. The length-bound check must
   refuse the frame before any body read is attempted.
3. **`rows.lock` path deleted while the fd is still held.** On Unix,
   the advisory lock is attached to the open file descriptor, not
   the path. Unlinking the path leaves the live `RowStore` entirely
   operational; writes keep committing and reads keep observing the
   committed state. This test documents the real behaviour.

Three new tests in `tests/fault_recovery.rs`:

- `wal_frame_with_length_zero_fails_closed`
- `wal_frame_with_length_above_maximum_fails_closed`
- `deleting_the_rows_lock_path_while_fd_is_held_keeps_the_store_live`
  (gated behind `#[cfg(unix)]`)

The Unix test is explicit that it **documents real behaviour**, not a
recommendation to unlink the lock file in production; after the live
store drops, a reopen of the directory still succeeds and still sees
every previously committed row.

## Properties this slice guarantees

- `len < 13` on a WAL frame is refused as Corruption with the
  "invalid WAL frame length" message before any body read.
- `len > MAX_WAL_BODY` (1 MiB) is refused as Corruption before any
  body allocation. **A corrupted header cannot force a large
  allocation.**
- Unix fd-held advisory locks survive path unlink; previously
  committed rows survive a drop-after-unlink reopen cycle.

## Remaining scope

- Real `ENOSPC`/`EIO` propagation at every call site, `fsync`
  failure on power loss, device-level torn writes, Windows and macOS
  qualification, remote and encrypted filesystems — all remain
  infrastructure-blocked.
- Lock-file behaviour on Windows is not qualified; the new Unix
  test is gated behind `#[cfg(unix)]`.

See [ADR 0052](../adr/0052-deterministic-fault-recovery.md),
[comprehensive gaps analysis](BRADIAN_COMPREHENSIVE_GAPS_ANALYSIS.md) §9,
[fixed roadmap](BRADIAN_MASTER_ROADMAP.md).
