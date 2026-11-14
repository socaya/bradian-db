# Master Gate 1 — Fault recovery matrix extension verification

14 November 2026. Baseline: `46b2ec1`.
Stable task G1-FAULTS in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**837 Rust tests passed, 0 failed, 0 ignored**. Clippy clean on
`bradian-relational --all-targets`. CI remains manual-only. No persistent
format, wire protocol or SQL grammar change.

| Executed check | Result |
|---|---|
| `cargo test --workspace` | 837 passed, 0 failed |
| `cargo test -p bradian-relational --test fault_recovery` | 9 passed (+3 new) |
| `cargo clippy -p bradian-relational --all-targets -- -D warnings` | Passed |

## Implemented and exercised

Three new tests extend the deterministic fault-recovery matrix past the
Slice 2 baseline:

- `index_file_missing_after_commit_rebuilds_without_corrupting_snapshot` —
  the derived index file is deleted after a committed checkpoint; the
  next open rebuilds the index from authoritative snapshot pages
  **without** rewriting the committed snapshot bytes.
- `backup_descriptor_missing_but_payload_present_is_detected_as_incomplete`
  — an archive directory contains payload files but no `backup.info`
  descriptor; restore refuses rather than replaying an unlabelled
  archive, and the destination is never published.
- `catalog_journal_truncated_mid_event_fails_closed_without_partial_catalog`
  — the WAL under `PersistentCatalog` is truncated mid-frame. Open
  either succeeds with exactly one event materialised (the one before
  the truncated frame) or returns a Corruption/Storage/Resource error.
  A partial multi-event catalog is never published.

No production code is modified; the tests exercise the real recovery
path on real files.

## Remaining scope

- Real `ENOSPC`/`EIO` propagation at every call site, `fsync` failure
  semantics on power loss, device-level torn writes, Windows and macOS
  qualification, remote and encrypted filesystems, long-running
  operational recovery rehearsals and multi-process shared-WAL /
  history-SQL fault matrices all remain open.

See [ADR 0052](../adr/0052-deterministic-fault-recovery.md) and the
[fixed roadmap](BRADIAN_MASTER_ROADMAP.md).
