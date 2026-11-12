# ADR 0013 — Crash-safe reconstruction and verification of derived physical indexes

**Product:** Bradian Database Systems · Master Architecture and SRS v1.1  
**Status:** Functional source and regression tests committed on `main`; manual compilation, fault injection and cross-platform qualification remain outstanding.  
**Prerequisites:** [ADR 0008 — WAL and checkpoint](0008-native-row-wal-checkpoint.md), [ADR 0010 — Physical B+tree](0010-checkpoint-btree.md), [ADR 0012 — WAL-backed delta](0012-wal-backed-mutable-index.md).

## Problem

A published `snapshot-<lsn>.brp` is authoritative; its derived `index-<lsn>.bri` is an accelerator. G5 publishes the snapshot before the index and before truncating WAL. If a process terminates after snapshot publication but before index publication, a legitimate durable checkpoint can exist without an index. The old code recovered the rows but would not reconstruct the missing index. Later `checkpoint()` calls could return early because the snapshot already existed.

Rebuilding an index by simply trusting previously stored pointers is unsafe. Recovery must derive every physical location directly from validated authoritative snapshot pages. It must detect malformed records, duplicate/unsorted keys, incorrect page identities, unexpected lengths and a derived index whose pointer contents disagree with the source. It must not quietly overwrite an existing corrupted index, because that would conceal evidence of storage damage.

## Decision

Introduce `bradian-relational::index_recovery` with a strict physical snapshot scanner and two recovery modes.

**Automatic recovery — absent index only:** On `RowStore::open()`, acquire the existing exclusive OS file lock, load and verify the latest authoritative snapshot, and detect whether its matching index file is missing. If missing, reconstruct the complete ordered key-to-physical-row-pointer mapping directly from the checkpoint's verified BRPG data pages. Build the matching B+tree into a separate `index-<lsn>.rebuild.tmp` file; verify its count and every indexed pointer; atomically publish as `index-<lsn>.bri`; synchronize the directory on Unix. Then replay any newer WAL frames and rebuild the committed in-memory delta. This guarantees the recovered index reflects **only** checkpoint records, not uncheckpointed changes.

**Explicit integrity audit:** `RowStore::verify_checkpoint_index()` scans authoritative snapshot records and requires the published B+tree to resolve each key to the exact matching page ID and payload offset. It verifies the index entry count as well. The operation is O(N log N) and deliberately remains an explicit administrative integrity check rather than an expensive requirement on every point lookup.

**Explicit missing-index reconstruction:** `RowStore::rebuild_missing_checkpoint_index()` is available under the same owning RowStore lock. It constructs an index only if the published index is absent. If an index already exists, the operation performs a full verification and refuses to overwrite a damaged index. The underlying rebuilding function is crate-private to reduce the risk of bypassing the RowStore's exclusive lock.

An existing damaged published index is **not automatically overwritten**. Such an incident requires an explicit, audited administrative repair design, evidence preservation and separate qualification. Normal reads through that existing index continue to fail closed where corruption is encountered.

## Physical recovery invariants

1. **Authority:** No index operation changes `rows.wal` or `snapshot-<lsn>.brp`; only derived index files and unpublished temporary files are created or removed.
2. **Durable generation:** The snapshot metadata page must contain the exact checkpoint LSN, row count and physical page count. Its length must equal the physical file length.
3. **Row integrity:** Every data page is CRC validated, must have the expected physical page ID and LSN, and all rows must have bounded lengths and globally strictly increasing binary keys.
4. **Pointer integrity:** Every reconstructed pointer is the actual byte offset of the length-prefixed record within its BRPG data page. An index whose CRC is valid but points to a different row/page fails the full verification.
5. **Publication:** Build to a new `.rebuild.tmp` file. `DiskBTree::build()` syncs the completed file. The rebuilt tree is fully queried/validated before rename. A successful publication syncs the directory on Unix.
6. **Interrupted rebuild:** A leftover `.rebuild.tmp` is never authoritative; reopening removes it and starts a fresh rebuild while holding the directory lock.
7. **WAL ordering:** Reconstruct the index from the checkpoint generation **before** WAL replay. WAL deltas are still replayed and merged with the immutable index by the G7 layer, preserving inserted, replaced and deleted rows after the checkpoint.
8. **No concealment:** A corrupt authoritative snapshot fails recovery; a corrupt existing index is not silently discarded or regenerated.
9. **Concurrency:** Rebuilding requires the RowStore exclusive lock and does not operate concurrently with a separate writable instance of the database.

## Tests and qualification

`crates/bradian-relational/tests/index_recovery.rs` covers automatic reconstruction after a missing publication, leftover temp files, newer WAL delta recovery, corrupted authoritative snapshots, a damaged existing published index, idempotent verification, empty checkpoints, missing snapshot refusal, wrong row pointers with valid reconstructed CRC, and preservation of authoritative snapshot/WAL bytes.

These are committed regression tests, not yet independently executed results. The owner has disabled automatic CI. Before G8 acceptance, run the crate and workspace test/lint commands and investigate failures. Additional production gates include filesystem power-loss injection (particularly Windows/macOS rename/directory sync semantics), large-index build memory and time limits, explicit damaged-index quarantine/replacement, secure audit trails, and checks against compromised storage outside the Bradian lock.

## Not implemented

This mechanism does not implement in-place WAL-protected B+tree page splitting/merging, transactional dirty-page eviction, MVCC, online nonblocking checkpoints, or multi-node consensus. It makes a specific checkpoint recovery boundary real and verifiable while keeping the source-of-truth architecture intact.
