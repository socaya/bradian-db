# ADR 0009 — Typed persistent table rows and primary-key integrity

**Status:** Implemented in source as a bounded relational table CRUD layer; manual test execution and production qualification outstanding.  
**Specification:** Bradian Master Architecture & SRS v1.1  
**Dependencies:** [ADR 0006 — generic catalog](0006-generic-relational-catalog.md), [ADR 0007 — physical page](0007-portable-physical-page.md), [ADR 0008 — row WAL and checkpoint](0008-native-row-wal-checkpoint.md)

## Decision

Introduce `bradian-relational::typed_table::TableStore` as a real, file-backed table CRUD API bound to an immutable `Table` definition issued by the generic catalog. It encodes values into a versioned `BRRW` record representation, derives composite primary keys in a versioned `BRPK` binary representation, and uses `RowStore` for synchronized WAL commits, checkpoints, restart recovery and exclusive directory ownership.

`TableStore::open(path, table)` must validate its table definition before accessing or creating data. The expected table structure is serialized as JSON into a reserved, durable metadata record within the same WAL-backed storage. Reopening the same directory with a mismatched table ID, column type, column ordering or constraint definition is rejected. An existing nonempty row store that lacks this metadata is rejected rather than silently recast as a table.

## Row representation

The v1 row record starts with `BRRW` (four bytes), `u8 version=1`, `u64 table_id` in little-endian, and a `u16 column_count`. Column values follow in the declared table order. Each value has a type tag:

| Tag | Value | Wire representation |
|---|---|---|
| 0 | NULL | No further bytes; permitted only for nullable columns |
| 1 | Boolean | Single byte 0 or 1 |
| 2 | Int32 | Four-byte little-endian signed integer |
| 3 | Int64 | Eight-byte little-endian signed integer |
| 4 | Decimal | Sixteen-byte little-endian signed unscaled integer, using column scale |
| 5 | Varchar/Text | Four-byte little-endian byte length followed by validated UTF-8 |
| 6 | TimestampTz | Eight-byte signed UTC microseconds since Unix epoch |
| 7 | UUID | Sixteen raw bytes |

Column validation occurs on both encoding **and** decoding. Decimal precision/scale, Varchar byte length, nullable columns, value tags, malformed Unicode, invalid Boolean encoding, table identity and trailing bytes are verified. The record must fit the current 3,546-byte inline RowStore value limit; larger values require a separately versioned overflow-page design.

## Primary-key representation

Each primary key starts with `BRPK` and includes a format version and stable table ID. Each key component uses the same typed, length-delimited scalar encoding as row values. A composite key of (`a`, `bc`) therefore differs from (`ab`, `c`). NULL primary keys are refused, key components must match their declared types, and encoded keys must fit the 512-byte RowStore key limit.

The encoding preserves **equality and deterministic identity**, but the byte-wise sort order is not yet guaranteed to match full SQL collation or numeric ordering. A B+tree key comparator and collation system require separate design and qualification.

## Data operations and concurrency

- `insert(row)` rejects an existing primary key, validates the complete row and records the value through `RowStore::commit_batch(Some(lsn), ...)`. Racing inserts cannot both succeed because the commit checks the observed global log position under the storage lock.
- `replace(row)` replaces only an existing row under the same primary key, and does not implicitly create a missing row.
- `delete(primary_key)` requires an existing row and commits a durable delete.
- `get(primary_key)` validates key shape/types and decodes a persisted row.
- `scan()` decodes all stored rows, rejects unrecognized namespaces, and checks that each stored key matches the row's encoded primary key.
- `checkpoint()` delegates to the page-backed snapshot and WAL-truncation protocol.

The current compare-and-swap commit rule protects the integrity of single table operations. It is not MVCC, predicate locking, SQL snapshot isolation or multi-table transactional atomicity. A globally contended LSN may cause valid requests to retry; users should not treat a retry exhaustion as a duplicate-key error.

## Explicit unsupported constraints

The catalog already models secondary UNIQUE keys and FOREIGN KEY definitions, but their transaction-safe **runtime** enforcement does not exist. For this reason, `TableStore::open` refuses tables declaring either. CHECK constraints, deferrable constraints, indexes, partitioning and triggers are outside this first typed table profile. Fail-closed rejection is mandatory; silently ignoring a declared constraint would risk inconsistent application data.

Likewise, the relational catalog itself is still in memory. Stable table IDs and immutable definitions must be supplied by the caller on each restart. Persistent, transactional catalog DDL and versioned online schema evolution remain the next engineering gates.

## Verification and release boundaries

The tests in `crates/bradian-relational/tests/typed_table.rs` cover persistent typed CRUD, duplicate PK rejection across restarts, schema mismatch, invalid types/null/precision/length, unsupported secondary constraints, deterministic row encoding, decoding malformed records, concurrent same-key insertion, composite-key collision safety and replacement/deletion preconditions.

The tests are committed, not presented as executed evidence. Automatic CI remains disabled under the repository owner's instruction. Manually run:

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

This release slice does not establish production readiness, broad SQL compliance, advanced isolation, durability after simulated physical power loss, or independent security certification.
