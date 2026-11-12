# BRD-G8 — Derived index reconstruction and checkpoint integrity audit

**Specification:** Bradian Database Systems Master Architecture and SRS v1.1  
**Architecture:** [ADR 0013](../adr/0013-derived-index-reconstruction.md)  
**Status:** Working code and regression tests committed to `main`; manual build/test and platform crash testing remain outstanding.

## Implemented source

The `bradian-relational::index_recovery` subsystem reconstructs physical index entries from fully validated checkpoint pages. It checks BRPG page IDs, LSNs, checksums, page count, record bounds and global binary key ordering. Each reconstructed index pointer records the actual page number and byte offset where the authoritative checkpoint contains the record.

When `RowStore::open` finds a valid published checkpoint without its derived B+tree, it rebuilds the missing index into a separate temporary file, verifies the complete leaf ordering and every point lookup, then publishes the index and syncs the directory on Unix. Any newer WAL frames are replayed afterwards, preserving the G7 mutable delta.

The `RowStore::verify_checkpoint_index()` API performs an explicit full audit of all indexed records against physical checkpoint rows. `rebuild_missing_checkpoint_index()` is a lock-protected repair of **missing indexes only**; an existing damaged index is reported instead of overwritten.

## Source test scenarios

- [x] Missing B+tree rebuilt after process restart from authoritative checkpoint.
- [x] Incomplete temporary index files ignored and replaced under the RowStore lock.
- [x] WAL changes after the snapshot remain visible after missing-index recovery.
- [x] Published snapshot corruption stops recovery; no derived index is trusted.
- [x] Existing corrupt published index is preserved and its verification fails.
- [x] Duplicate rebuild requests do not overwrite valid published indexes.
- [x] Empty checkpoint index reconstruction is supported.
- [x] No checkpoint means rebuild and verification are refused.
- [x] CRC-valid but physically incorrect row-pointer values fail the integrity audit.
- [x] Rebuilding a derived index does not modify the authoritative snapshot/WAL bytes.

Checkboxes mean **tests have been committed**, not that they have been executed successfully.

## Verification required

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

System integration also requires Linux/macOS/Windows power-loss testing, storage fault injection, fsync/rename guarantees, stress tests for very large checkpoint indexes and explicit administrative quarantine/repair of corrupt existing indexes.

## What this is not

This is not an in-place mutable B+tree, WAL-logged index-page split/merge, dirty-page write-back engine, MVCC manager or general-purpose SQL transaction executor. The published physical B+tree remains immutable between checkpoints, while G7 provides incremental committed WAL changes. Full physical transactional page management remains a separate architectural and acceptance milestone.

Automatic CI remains disabled at the owner's request.
