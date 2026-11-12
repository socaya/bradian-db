# ADR 0010 — Derived page-based B+tree for native row checkpoints

**Product:** Bradian Database Systems  
**Specification:** Master Architecture and SRS v1.1  
**Status:** Implemented as an immutable disk-resident B+tree; manual compile/test and operating-system qualification remain outstanding.  
**Dependencies:** [ADR 0007](0007-portable-physical-page.md), [ADR 0008](0008-native-row-wal-checkpoint.md), [ADR 0009](0009-typed-table-rows.md)

## Background and architectural decision

The native row store already records authoritative mutations in a CRC-protected synchronized WAL and periodically builds a sorted, checksummed physical-page checkpoint. Previously, its primary-key lookup relied exclusively on an in-memory `BTreeMap` reconstructed at startup, so no durable physical index existed.

The next incremental design adds a **real multi-level, disk-resident B+tree**, rather than introducing a dummy index interface. The B+tree is an independently validated *derived* structure rebuilt from each published checkpoint. It maps raw binary keys to the physical location of the corresponding record in `snapshot-<lsn>.brp`. The WAL and snapshot remain authoritative; no uncommitted or stale mutation can be made visible through the index.

This approach intentionally separates page-tree correctness from in-place splitting and write-ahead page mutation. A future mutable B+tree and buffer pool will require its own rigorous latch, WAL-before-data, page reuse, split-recovery and MVCC protocol.

## On-disk page format

Index files are named `index-<lsn>.bri`, with a construction file `index-<lsn>.tmp`. They are sequences of exactly 4096-byte `BRPG` physical pages protected by the existing page checksums.

| Page | Type | Contents |
|---|---|---|
| 0 | `Metadata` (4) | `BRIX` magic, u16 format version 1, u64 root page ID, u64 node count, u64 indexed row count |
| 1..N — leaf | `Index` (2), node tag 1 | u16 entry count, u64 next-leaf page, repeated (u16 key length, u64 data page ID, u16 row offset, key bytes) |
| 1..N — internal | `Index` (2), node tag 2 | u16 separator count, u64 leftmost child, repeated (u16 separator-key length, u64 right child ID, separator-key bytes) |

Integers are explicitly little-endian. Leaf nodes are filled in strictly ascending key order. Each internal separator is the smallest key of its right child subtree. Child page references point to immutable index pages; leaf row pointers reference the accompanying row checkpoint, never another file generation. An empty tree has root ID 0, row count 0 and no index nodes.

A physical row pointer consists of the snapshot data page ID and a byte offset into that page's payload. On lookup, Bradian reads and verifies the referenced `BRPG` data page, checks the row bounds and verifies that the stored key is precisely the requested key before exposing its value. This ensures that a physically corrupt pointer does not silently become an incorrect result.

## Building and publishing the tree

The row checkpoint writer already iterates records in ascending key order. It now records each row's exact physical data-page ID and payload offset while streaming immutable row pages to the checkpoint. This produces a sorted sequence of key-pointer pairs without duplicating row values.

The B+tree builder packs leaf pages according to the physical page capacity and links them in ascending order. It then repeatedly groups lower-level page descriptors into internal pages until one root remains. It writes a versioned metadata page 0 and synchronizes the new index file.

Checkpoint publication follows this order:

1. Serialize the authoritative row checkpoint to `snapshot-<lsn>.tmp` and synchronize the file.
2. Publish the immutable `.brp` snapshot and synchronize the directory.
3. Build the derived index into `index-<lsn>.tmp`, synchronize it, rename to `.bri` and synchronize the directory.
4. Only after both structures have been safely published, truncate and synchronize the redundant WAL.
5. Opportunistically delete older checkpoint and index generations.

A crash between steps 2 and 3 leaves an authoritative snapshot without an index. Recovery still works because an absent index is treated as an optional accelerator; direct row reads remain available. A crash after index publication but before WAL truncation is safe because WAL replay ignores operations already included at the checkpoint LSN.

## Query routing and correctness

`DiskBTree::get()` performs real page reads and branch traversal until it reaches the appropriate leaf. `DiskBTree::range()` seeks to a leaf and follows its next-leaf links for bytewise ordered range retrieval. Pages are CRC validated at read time and reference, ordering and byte-bound checks are enforced.

`RowStore::indexed_get()` and `RowStore::indexed_range()` use the physical index only when the latest commit LSN equals the checkpoint LSN. When WAL mutations are newer, they return committed in-memory values and never claim the immutable index includes newer data. As a defensive correctness condition, indexed results are compared with the reconstructed authoritative row map. The typed table's primary-key `get()` uses this indexed read path automatically.

The present bytewise key ordering does **not** claim to implement SQL collation order or numeric ordering for little-endian values. The typed key format guarantees equality and nonambiguous composite keys; a SQL-correct B+tree comparator and locale/collation versioning will be a separate contract.

## Buffer pool integration (ADR 0011)

The original G5 implementation performed direct physical reads on each lookup. The new [generation-scoped buffer pool](0011-generation-scoped-buffer-pool.md) retains verified B+tree and snapshot pages in bounded LRU caches. These caches are reused across point/range lookups of one immutable checkpoint generation, and are invalidated when a new checkpoint is published. The WAL-backed committed row map still overrides the older checkpoint index whenever there are newer writes.

This is a read-only physical cache; it does **not** implement dirty-page flushing, page splits in place or buffer-pool latching. The B+tree remains a derived immutable index over the authoritative published snapshot.

## Safety, scale and limitations

The tree is genuinely persisted and queried from disk. However, it is **not** a fully mutable, concurrent, disk-first transactional B+tree. Changes after a checkpoint are held in the WAL-backed in-memory map until the next checkpoint rebuild; no in-place page split, merge, free-page reuse, buffer-pool caching, page-level latching or incremental secondary index maintenance is claimed.

The index is disposable only when the row snapshot and WAL are intact. A checksum or structural failure in an existing index query is reported instead of presenting an absent row. Repair/rebuild utilities will require independent design; automatic silent repair must never conceal a corrupted authoritative snapshot.

At this stage, checkpoint construction holds the store's in-process mutex, and the B+tree builder retains one key-pointer entry per row transiently. Both constraints require benchmarking and online-compaction design before claiming low-latency multi-terabyte operation. Failure of index publication leaves the RowStore poisoned until restart; the WAL is not truncated before successful checkpoint completion.

## Source evidence

- `crates/bradian-relational/src/btree.rs` — physical B+tree construction, root metadata, branch traversal and linked-leaf range scans.
- `crates/bradian-relational/src/row_store.rs` — pointer collection, index publication, checkpoint agreement and indexed reads.
- `crates/bradian-relational/src/typed_table.rs` — primary-key reads through the indexed path.
- `crates/bradian-relational/tests/btree_index.rs` — empty, single-level and deep trees, range operations, page corruption, index mismatch, fresh WAL fallback and pruning.
- `crates/bradian-relational/tests/typed_table.rs` — typed primary-key index use and corruption propagation.

**Verification:** The source and tests are committed on `main`. Per current repository policy, automatic CI is disabled and no Cargo test result is claimed. Manual compile, test, lint and crash/fault-injection tests are required before accepting this gate.
