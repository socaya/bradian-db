# ADR 0025 — MVCC row-version visibility and per-key optimistic commits

**Product:** Bradian Database Systems · Master Architecture & SRS v1.1  
**Status:** G20 code and integration tests committed to `main`; manual Cargo compilation, test execution, and failure-injection qualification still required.  
**Dependencies:** [ADR 0008 — WAL](0008-native-row-wal-checkpoint.md), [ADR 0018 — single-table optimistic transactions](0018-single-table-transactions.md), [ADR 0020 — shared-WAL multi-table domain](0020-shared-wal-multi-table-atomicity.md).

## Problem

G13/G15 transactions clone an entire row map when they begin and reject writes whenever **any** committed mutation has advanced the table/domain's global WAL LSN. This gives a conservative one-store conflict model but unnecessarily conflicts on disjoint keys. The prototype lacks a row-version visibility horizon, tracking of concurrent readers, a version reclamation policy or a per-key first-committer-wins rule. A direct switch to general SQL MVCC would be unsafe without a durable version format and a qualified recovery protocol.

## G20 decision

Introduce an **opt-in**, standalone `bradian-relational::mvcc::MvccStore` wrapping one exclusive native G3 `RowStore`. It is a genuine in-process versioned-row transaction slice and **not yet wired into the existing G16 DomainSqlEngine**. Existing G15/G16 SQL transactions retain their conservative whole-domain conflict behavior.

On open, G20 reconstructs the latest committed key/value state from the authoritative BRPG checkpoint and BRWL replay. Each current key is represented as an in-memory chain of `Version { lsn, value }`, where `None` denotes a committed deletion tombstone. Earlier historical versions cannot be reconstructed from G3's current-row physical format and therefore **are not available across a process restart**.

`MvccStore::begin()` pins the current committed LSN in a reference-counted `BTreeMap<u64,usize>`. It does not clone the entire row set. Readers look up the most recent version satisfying `version.lsn <= start_lsn`, combined with their own staged changes. A transaction can read a stable old value while another concurrently commits a replacement, deletion or insertion.

## Write and conflict protocol

A transaction stages `insert(key,value)`, `replace(key,value)`, and `delete(key)` in a sorted in-memory map; duplicate INSERT and missing-key replacement/deletion are errors based on transaction-visible state. No staged changes are published to concurrent readers before commit. Dropping or calling `rollback()` discards staged mutations and releases the snapshot pin without writing WAL.

On commit:

1. Lock the MVCC history and verify its latest LSN agrees with the underlying RowStore. No other caller is allowed to mutate this RowStore directory while the MVCC façade owns its exclusive file lock.
2. If the transaction performed a full-table `scan()`, require that the **global** committed LSN remains unchanged since the transaction began. This conservative rule prevents the full scan from making a write based on untracked phantoms or range changes.
3. For a non-scanning writer, check **every modified key**: reject when its latest retained committed version has `lsn > start_lsn`. This is first-committer-wins per key, and detects delete/reinsert ABA cycles while an older transaction is pinned.
4. Convert the final distinct-key mutation map into one native `RowStore::commit_batch(Some(current_lsn), ...)`. This uses G3's existing validation, CRC-protected atomic frame, synchronized WAL append, and maximum of 256 key operations / 1 MiB body. The globally serialized WAL commit receives a single next LSN, even when two transactions started from the same earlier LSN and modified **disjoint** keys.
5. **Only after** the WAL has synchronized successfully, add all new in-memory key versions at the returned LSN, update the current committed LSN, release the writer's snapshot pin and reclaim unreachable versions. A failed commit does not publish any staged history.

Because the history mutex serializes the check, native WAL commit and version publication, readers never see only part of a committed multi-key transaction.

The per-key rule permits disjoint writers from the same initial snapshot to commit, unlike G15. It also permits classic **write skew across different keys**, unless a transaction uses the conservative full-scan path. G20 is therefore **not serializable**, and it does not support SQL predicate locks, range reads or configurable isolation levels.

## Snapshot retention and garbage collection

The oldest active transaction LSN pins the latest version at or before that position for each key, plus every later version. This protects every active reader's visibility. When the oldest reader releases its pin, versions no longer required by any active snapshot are reclaimed. If there are no readers, the engine retains one latest value per key and can discard standalone deletion tombstones.

The `MvccStore::stats()` diagnostic reports the committed LSN, active snapshot count, number of distinct pinned LSN positions, number of retained keys and retained versions. Long-lived reader transactions may cause proportional memory growth; **G20 has no spill-to-disk version history, resource admission control, or persistent historical-version vacuum**.

`checkpoint()` writes the latest authoritative committed key/value state through existing BRPG/BRCP machinery while keeping active historical versions in RAM. A restart recovers the newest committed values and LSN only; it must never claim to resurrect snapshots from a prior process.

## API profile

```rust
let store = MvccStore::open(root)?;
let mut a = store.begin();
let mut b = store.begin();

a.insert(b"account-a", b"100")?;
b.insert(b"account-b", b"200")?;

a.commit()?;
b.commit()?; // both may succeed: disjoint-key first-committer-wins

let mut reader = store.begin();
let rows = reader.scan(); // full-table scan is stable for this snapshot
reader.rollback();
```

`MvccTransaction` exposes `start_lsn`, `get`, `scan`, `insert`, `replace`, `delete`, `rollback` and `commit`. The `RowStore` is private: source code using this façade cannot directly bypass MVCC version bookkeeping.

## Integration boundary and next work

- G20 is a **standalone single-RowStore MVCC prototype**, not MVCC-enabled SQL sessions. A subsequent gate must bind typed schema pins, table identities, SQL statements, DDL versioning and query plans to versioned storage without mixing legacy G15 writes.
- Physical row/version chains are in memory; current rows alone are WAL-durable. Durable time travel and crash-stable old snapshots require a **versioned on-disk page or journal format**, generation-aware GC and an explicit compatibility/migration protocol.
- Version retention is tied to in-process pinned transactions, not long-running network sessions or distributed snapshot tokens.
- Point writes follow first-committer-wins, but write skew remains possible. Full scans use coarse whole-store validation, not predicate or key-range locks.
- No secondary UNIQUE/FK/CHECK constraints, MVCC indexes, serializable transactions, group commit, deadlock detection, optimistic read sets, transaction timeout enforcement, encryption or cluster-wide isolation are provided.
- Performance remains constrained by in-memory versions and the G3 atomic WAL batch size. Independent benchmarking is required before claiming OLTP concurrency gains.

## SQL typed integration update (ADR 0026)

[G21](0026-typed-sql-mvcc-integration.md) adds a separate `MvccSqlEngine` using durable G11 catalog identities and validated G15 table schema pins with G20 per-key MVCC snapshots. It reuses the G16 parser and permits overlapping in-process SQL sessions without changing G16's legacy conflict semantics. G21 does not persist old versions or provide serializable isolation.

## Durable history extension (ADR 0027)

[G22](0027-durable-mvcc-history.md) introduces a *separate*, opt-in `BRADIAN-DURABLE-MVCC-V1` version-key storage format that persists historical values and tombstones in the same synchronized G3 WAL batch. Historical point reads can survive restart within a durable retention floor. This does not alter G20's in-memory history behavior or silently migrate G21 SQL.

## Evidence and qualification

`crates/bradian-relational/src/mvcc.rs` implements pinned version chains, durable commits, conflict detection, read-your-writes and reclamation.

`crates/bradian-relational/tests/mvcc.rs` provides 14 source-level integration tests for snapshot stability, disjoint writer commits, same-key conflicts, ABA detection, full-scan phantom protection, rollback/drop, checkpoint/restart, multiple reader pins, memory reclamation, duplicate/missing-key errors, oversized batch refusal, multithreaded writes and read-only snapshots.

**Source and tests have been committed; no passing Cargo test result is claimed.** Automatic GitHub CI remains manual-only at the owner's request. Manual `cargo check --workspace --locked`, `cargo test -p bradian-relational --locked`, workspace tests, Clippy and WAL crash fault injection are release prerequisites.
