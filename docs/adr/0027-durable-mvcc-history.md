# ADR 0027 — Durable version-key MVCC history and retained as-of reads

**Bradian Database Systems · Master Architecture and SRS v1.1**  
**Status:** G22 source and regression tests committed to `main`. Compilation, test execution, filesystem fault injection and release qualification remain outstanding.  
**Dependencies:** [ADR 0008 — native WAL](0008-native-row-wal-checkpoint.md), [ADR 0015 — backup/restore](0015-consistent-backup-restore.md), [ADR 0025 — in-memory MVCC](0025-mvcc-row-version-foundation.md), [ADR 0026 — typed SQL MVCC](0026-typed-sql-mvcc-integration.md).

## Problem and compatibility decision

G20/G21 preserve historical row versions in memory while active transactions need them, but the authoritative G3 RowStore persists only the latest state of each physical data key. After a restart, the G21 SQL engine recovers current committed values and cannot offer historical `AS OF` reads. A separate history file would introduce a dangerous commit-order problem: a crash could persist the current-row WAL but lose its history record, or persist history for a transaction that never committed.

G22 implements a new, **explicitly opt-in** version-key RowStore format, `DurableMvccStore`, where history and the transaction commit decision are the **same native BRWL batch**. It does not reinterpret existing G20/G21 stores, silently introduce new keys into the G15/G16 typed domain, or claim that SQL sessions are already using durable history. Converting the G21 typed MVCC engine requires a separate migration/adapter gate.

## Physical format

A G22 directory is a standard exclusive G3 RowStore whose committed rows include:

| Key namespace | Value |
|---|---|
| `\0bradian.durable-mvcc.format` | `BRADIAN-DURABLE-MVCC-V1` marker |
| `\0bradian.durable-mvcc.floor` | 8-byte big-endian minimum retained history LSN |
| `DVH1 + u16_BE(key_length) + user_key + u64_BE(commit_lsn)` | `0` tombstone or `1 + value_bytes` committed value |

The marker and initial floor 0 are atomically created in one synchronized WAL batch (initial LSN 1). The composite history key makes each committed version immutable and sorts versions of a given user key by increasing LSN. User keys are nonempty and at most 498 bytes, producing physical keys at most G3's 512-byte maximum. User values are at most 3545 bytes (plus one presence marker), under G3's inline capacity.

Every subsequent committed transaction creates one `DVH1` history record **per distinct modified logical key**, all inside one synchronized G3 WAL batch at a single commit LSN. Deletion is represented by an immutable tombstone record rather than by removing an older version. The existing G3 BRWL checksum, fsync, immutable BRPG checkpoint, BRCP manifest, derived B+tree, backup and replay are reused without modifying their formats.

## Visibility and transaction semantics

`DurableMvccStore::begin()` pins the current durable WAL LSN. A transaction performs `get`, `scan`, staged INSERT/replace/delete and can `commit` or `rollback`.

- `get` and `scan` return the latest version whose commit LSN is at most the transaction's pinned starting LSN, plus the caller's staged writes. The history index is held in memory while the store is open but reconstructed from durable `DVH1` records on restart.
- A non-scanning writer can commit alongside concurrent writers changing unrelated user keys, using first-committer-wins checks against each modified key's last retained version. Same-key updates, including delete/reinsert ABA, are rejected when their last version is newer than the transaction start.
- If the transaction scanned the entire store, it uses conservative global-LSN validation before writing to avoid untracked full-scan phantoms. This is **not** general serializable isolation and can still permit write skew from point reads.
- WAL commit is serialized under the history mutex, and all new version records become visible in memory **after** one successful synchronized RowStore batch append. A failed or partial BRWL frame cannot publish only part of the transaction.
- Read-only transactions return the starting LSN without modifying WAL. Dropping a transaction or calling `rollback` releases its pin and writes nothing.
- The existing G3 256-operation and 1 MiB WAL batch limits apply. Versions beyond these limits are rejected, not partially published.

The source of truth is the WAL/checkpoint combination. On restart, `DurableMvccStore::open` refuses unknown key namespaces, unrecognized marker, invalid version encoding, duplicate composite commit LSNs or version commit positions beyond the recovered G3 WAL LSN. No previous-process live transaction handle or pin is resurrected.

## Retention and restart-safe reclamation

G22 exposes explicit `prune_through(lsn)`. It first checks the requested floor is nondecreasing, no greater than the latest committed LSN and **does not pass an active pinned snapshot**. The floor is synchronized as a native WAL record **before** any history key is deleted. Consequently, a crash during garbage collection cannot make a previously advertised history horizon silently return incorrect values: reads below the durable floor are explicitly rejected.

For every logical key, pruning retains the newest committed version at or before the floor as the visibility baseline, plus every version newer than the floor. Earlier superseded versions are removed in bounded batches of at most 256 native row-key deletes. If deletion is interrupted, reopening reconstructs the still-retained versions and the durable floor; the operator may safely rerun pruning at the same floor to complete cleanup.

The floor represents the **earliest permitted query horizon**, not a guarantee that every key has a version committed at exactly that LSN. A key with no version before the queried LSN correctly reads as absent. Tombstone baselines are retained as needed for correct deletion and reinsertion visibility. No automatic retention time window, memory quota, spill-to-disk, disk-space budget or background vacuum is supplied.

**Critical scope:** G22 preserves queryable historical data from committed WAL events after restart **while the chosen retention floor allows those queries**. It does not support transactions resuming across process restarts, and historical version retention can still consume substantial disk space.

## Backup and restore

`DurableMvccStore::backup_to` invokes G10's synchronized physical backup while holding the version-state mutex. The backup includes the format marker, floor, all retained history keys, WAL and checkpoint generation. `restore_backup` restores through G10 only into an absent destination; a fresh `DurableMvccStore::open` then checks the G22 format and reconstructs exact retained history. This is not yet part of G19's separate three-component SQL database archive.

## API example

```rust
use bradian_relational::durable_mvcc::DurableMvccStore;

let store = DurableMvccStore::open("/data/bradian-history")?;
let mut tx = store.begin();
tx.insert(b"customer-1", b"value-v1")?;
let first_version = tx.commit()?;

let mut tx = store.begin();
tx.replace(b"customer-1", b"value-v2")?;
tx.commit()?;

assert_eq!(
    store.read_as_of(b"customer-1", first_version)?,
    Some(b"value-v1".to_vec())
);

store.checkpoint()?;
```

The `first_version` LSN remains queryable after a restart unless a later authorized retention operation advances the floor beyond it.

## Security, correctness and remaining work

G22 uses exclusive single-process storage locks and no network protocol. It does not implement schema-aware SQL execution, per-key physical indexes optimized for historical-range lookups, consistent as-of analytics across catalogs and domain schemas, persisted transaction sessions, automatic version garbage collection, encrypted archives, cryptographic authentication or cluster coordination.

G22's historical version-key format is intentionally **incompatible with direct opening by G21's original typed-row parser**. A future gate must coordinate catalog schema pins, G15 data migration, G19 backup semantics, typed historical reads and SQL grammar without mixing two independent writers or silently discarding data.

## Verification evidence

Implementation is `crates/bradian-relational/src/durable_mvcc.rs`; regression tests are in `crates/bradian-relational/tests/durable_mvcc.rs`. The tests include durable historical values after checkpoint/restart, tombstone/reinsert history, disjoint writers, same-key conflicts, scan conflicts, rollback, retention-floor persistence, pinned reader protection, GC baseline visibility, unknown-format rejection, WAL batch limits, multithreaded writers, read-only snapshots, G10 backup/restore and key/value bounds.

**The source and tests are committed, but no Cargo build or test result has yet been observed.** Manual workspace checks, WAL/power-loss fault injection, long retention benchmarks and platform-specific storage validation are required. Automatic GitHub Actions remain disabled per owner direction.
