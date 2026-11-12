# BRD-G9 — Atomic checkpoint publication and generation recovery

**Specification:** Bradian Database Systems Master Architecture & SRS v1.1  
**Architecture:** [ADR 0014](../adr/0014-atomic-checkpoint-manifest.md)  
**Status:** Operational Rust source and regression tests committed to `main`; acceptance remains open pending manual build and fault testing.

## Delivered vertical slice

A new `bradian-relational::checkpoint_manifest` module introduces a fixed-size versioned, CRC-protected `BRCP` checkpoint generation marker. The RowStore now makes a checkpoint authoritative only after both its immutable BRPG snapshot and derived B+tree have been synchronized and the manifest published atomically. Redundant WAL records are not truncated until this point.

On recovery the manifest identifies the committed snapshot. A newer snapshot filename left by an interrupted publication is not trusted. Missing published index files are reconstructed from verified authoritative pages using the G8 implementation; newer WAL changes are replayed and merged using the G7 delta.

When a previous interrupted checkpoint left conflicting filenames for the target LSN, the store quarantines uncommitted files using `.orphan` suffixes under its exclusive lock rather than overwriting them silently. Existing invalid manifests and corrupted authoritative snapshot files fail closed. Pre-manifest stores retain a read-compatible legacy snapshot selection path until their first successful new checkpoint.

## Regression coverage committed

- [x] Exact 40-byte BRCP format roundtrip and CRC failure detection.
- [x] Successful manifest publication with row, snapshot-page and index-page counts.
- [x] Restart using manifest-designated committed generation.
- [x] Newer orphan snapshot filename ignored while WAL recovers subsequent writes.
- [x] Orphan conflict quarantine and successful checkpoint retry.
- [x] Corrupt or missing manifest-designated snapshot refuses startup.
- [x] Interrupted `checkpoint.pending` ignored.
- [x] Pre-manifest checkpoint compatibility.
- [x] Manifest sequence cannot regress.
- [x] Missing index under committed manifest rebuilt from verified rows.

These checkboxes represent **source test cases**, not executed test evidence.

## Manual acceptance commands

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

## G9 release qualifications still open

- [ ] Compilation and committed test suite executed successfully on Linux, macOS and Windows.
- [ ] Fault-injection across file write, sync, rename, manifest publication and WAL truncation.
- [ ] Persistent dirty-page buffer pool with WAL-before-data ordering and bounded eviction.
- [ ] Incrementally mutable on-disk B+tree page splits, merges and page reclamation.
- [ ] Torn-page and disk-full recovery qualification.
- [ ] Backup/restore integration with manifest-generation consistency and index rebuilding.
- [ ] Multi-statement ACID/MVCC, lock isolation and SQL transactional semantics.

**Do not describe BRD-G9 as a completed mutable physical page engine.** The committed slice implements an explicit crash-recovery publication boundary for immutable checkpoint generations. Full physical transactional page mutation remains a future milestone.

Automatic CI remains disabled by owner request.
