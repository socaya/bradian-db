# ADR 0014 — Atomic checkpoint manifest and safe physical generation selection

**Product:** Bradian Database Systems · Master Architecture & SRS v1.1  
**Status:** Implemented as a focused crash-boundary source slice, with regression tests committed to `main`. Manual compilation, fault injection and cross-platform qualification remain outstanding.  
**Depends on:** [ADR 0008](0008-native-row-wal-checkpoint.md), [ADR 0010](0010-checkpoint-btree.md), [ADR 0013](0013-derived-index-reconstruction.md).

## Context

Bradian already writes CRC-checked 4 KiB row snapshot pages, derives a multi-level index, synchronizes both, then truncates redundant WAL. The previous recovery selection relied on the numerically highest `snapshot-<lsn>.brp` filename. That is insufficient to distinguish a deliberately committed checkpoint generation from files left by a process that stopped partway through publishing a snapshot, index or directory entry.

A full physical page transaction manager will ultimately require WAL-before-data ordering, redo/undo, page-level versioning, dirty-page flush and torn-page recovery. This ADR implements a narrower but important prerequisite: **explicit checkpoint generation commit**, without introducing unproven in-place updates to authoritative pages.

## Decision

Introduce a fixed-size, checksummed `checkpoint.current` manifest. Only its designated generation is authoritative whenever a manifest exists. A prior database with no manifest continues to use the historical highest published snapshot for compatibility until its next successful checkpoint.

### Manifest binary format (40 bytes)

| Offset | Width | Field |
|---|---:|---|
| 0 | 4 | ASCII `BRCP` |
| 4 | 2 | Major version 1, little-endian |
| 6 | 2 | Reserved zero bytes |
| 8 | 8 | Committed checkpoint LSN |
| 16 | 8 | Committed row count |
| 24 | 8 | Snapshot data-page count |
| 32 | 4 | Derived B+tree index-page count |
| 36 | 4 | CRC32 over bytes 0–35 |

All fields have a defined binary representation independent of host endianness. A malformed or CRC-failing existing manifest is a hard error: the database must not silently switch to legacy snapshot selection. The manifest's expected snapshot and index file lengths are checked against physical page size, while existing BRPG readers separately verify every physical page checksum and the snapshot's metadata contents.

## Checkpoint write protocol

Under the RowStore's existing exclusive file lock and process mutex:

1. Validate committed in-memory state and serialize snapshot pages into `snapshot-<lsn>.tmp`.
2. Synchronize the snapshot file, rename to immutable `snapshot-<lsn>.brp`, and synchronize the directory on Unix.
3. Build the matching B+tree in `index-<lsn>.tmp`, verify/synchronize its pages, rename to `index-<lsn>.bri`, and synchronize the directory.
4. Construct the 40-byte manifest from the exact published snapshot/index lengths, checkpoint LSN and row count.
5. Write `checkpoint.pending`, synchronize it, atomically rename it to `checkpoint.current`, and synchronize the directory on Unix. The new checkpoint generation becomes authoritative **only at this publication boundary**.
6. Only after publication succeeds: truncate/synchronize redundant WAL, clear the mutable WAL index delta, invalidate the G6 page caches, and opportunistically remove obsolete snapshots and index files.

Step 5 must precede WAL truncation. If the process crashes while an uncommitted newer snapshot exists, the older manifest remains authoritative and WAL replay recovers the new row mutations. If the process crashes after manifest publication but before WAL truncation, recovery loads the new committed checkpoint and ignores WAL records already represented by its LSN.

## Restart and orphan handling

`RowStore::open` reads the manifest before choosing a snapshot. If one exists, it requires exactly `snapshot-<lsn>.brp` and verifies its physical length, CRC-bearing pages, metadata, row count and data-page count. It then reconstructs a **missing** derived index through G8, or verifies the index file length if present, before WAL replay.

A newer snapshot whose filename exceeds the manifest LSN is **not** automatically selected. During a later checkpoint for that same newer LSN, the owning RowStore moves conflicting uncommitted snapshot/index filenames to `.orphan` quarantine and writes a freshly validated generation. The quarantined bytes are retained for administrative inspection instead of silently deleted. An existing quarantine name is a hard conflict; the operator must investigate rather than overwrite prior evidence.

Incomplete `checkpoint.pending` files are ignored during recovery; the next publisher replaces them under exclusive ownership.

## Correctness boundaries and remaining risks

- This is a source-of-truth **generation publication** mechanism, not a claim of arbitrary SQL transaction durability or page-level WAL redo/undo.
- Published snapshot pages and their derived B+tree are still immutable. The G7 WAL delta holds changes until the next checkpoint, preserving atomic batch operations.
- A manifest CRC is accidental corruption detection, not a cryptographic signature. A hostile actor able to rewrite files and checksums is out of scope until storage authentication and key management are implemented.
- On Unix the implementation synchronizes directory metadata after rename; Windows/macOS and unusual filesystems require separately qualified atomic replacement and flush procedures. `checkpoint.current` should not be presented as universally power-loss-safe without such testing.
- The checkpoint still holds a coarse row-store mutex and the full committed row map in RAM. Large checkpoint performance, multi-process write sharing and distributed coordination are not solved.
- Replacing an existing checkpoint manifest is an important durability operation; operational backup must capture a consistent manifest, snapshot, derived index and corresponding WAL boundary.
- Quarantined orphan generations are not automatically removed or restored. Operators need a future verified maintenance command and forensic audit trail.

## Tests

`crates/bradian-relational/tests/checkpoint_manifest.rs` covers format roundtrips, CRC and version rejection, successful published generations, an orphan higher-numbered snapshot being ignored and safely quarantined, corrupt/missing authoritative metadata, interrupted pending writes, compatibility with pre-manifest snapshots, LSN regression rejection and derived-index reconstruction for manifest-selected generations.

The tests are committed but were **not executed** in this task. Automatic CI remains disabled by owner request. Manual `cargo check`, `cargo test`, Clippy, Linux/macOS/Windows power-loss injection and disk-full fault simulation remain required before accepting G9.
