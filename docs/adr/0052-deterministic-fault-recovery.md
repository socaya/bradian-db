# ADR 0052 — Deterministic row-store fault recovery (G1-FAULTS)

**Status:** Accepted for the failure outcomes listed below. Gate 1 remains active.
This does not accept the full master G1-FAULTS exit (real power-loss, disk-full
on physical media, Windows/macOS device qualification).

Existing coverage already exercises CRC corruption, torn WAL tails, truncated
record bodies, manifest regression, interrupted `checkpoint.pending`, orphan
snapshot filenames, pre-manifest legacy stores and missing index after a
committed manifest. The G1-FAULTS entry remains partial for failure outcomes
that leave distinct on-disk artifacts a crash can plausibly produce.

The new tests cover these outcomes by direct filesystem manipulation between
`RowStore` drops. No fault-injection abstraction is introduced into production
code; the tests exercise the real recovery path on real files.

1. **Zero-padded WAL extension.** After the last complete frame, the WAL file is
   extended with NUL bytes (the outcome that a short `write_all` under
   ENOSPC can leave behind when the kernel grew the file before erroring). Recovery
   must fail closed with `StoreError::Corruption("bad WAL magic…")` rather than
   silently trim a zero-magic header as "incomplete".

2. **Both `snapshot-<lsn>.tmp` and `index-<lsn>.tmp` orphans coexist** at a
   higher LSN than the committed generation. The next open must use the
   published generation (via the manifest) and leave the `.tmp` artifacts
   alone — they are not part of any committed state.

3. **Multiple aborted `.tmp` generations at different LSNs** (two separate
   interrupted checkpoints). Recovery must still return the committed
   generation and must not mistake the highest-numbered `.tmp` artifact for an
   authoritative snapshot.

4. **Unrelated incidental files** (`.DS_Store`, `README`, stray `foo.bak`) in
   the store root must not confuse recovery or the pre-manifest listing path.
   The directory-admission policy (ADR 0051) still applies; these files count
   toward the default budget.

5. **Zero-filled WAL at an incomplete body** — a complete header declaring a
   body followed by all-zero body bytes whose CRC does not match. Recovery must
   fail closed with `StoreError::Corruption("WAL CRC mismatch…")`.

No production code is modified; no persistent format, wire protocol or SQL
semantic changes. Recovery messages are checked at the `StoreError` variant
level so that future wording changes do not spuriously break the suite.

Still open: real `ENOSPC`/`EIO` propagation at every call site, `fsync` failure
semantics on power loss, Windows and macOS device-level qualification, remote
and encrypted filesystems, long-running operational recovery rehearsals,
multi-process failures and shared-WAL/history-SQL fault matrices.
