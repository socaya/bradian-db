# Bradian Native Relational Storage

**Package:** `bradian-relational`  
**Target specification:** Bradian Database Systems Master Architecture & SRS v1.1

This crate contains three operational building blocks that remain separate from the legacy DAP analytical engine and DHIS2-specific schema semantics:

1. **Generic relational schema catalog.** In-memory databases, schemas, typed columns, primary and unique-key definitions, foreign-key metadata and dependency-restricted object removal.
2. **Portable physical pages.** The versioned `BRPG` 4096-byte binary page codec, explicit little-endian fields, CRC32 integrity verification, and strict decoding.
3. **Durable single-process row storage.** `RowStore` performs individually atomic insert/replace/delete operations and bounded multi-key WAL batch commits, syncs the `BRWL` before publishing each write, loads `.brp` checkpoint pages, replays committed log records on restart, repairs a truncated *final* WAL frame, and rejects corruption.

## Running the implementation

```rust
use bradian_relational::row_store::RowStore;

fn example() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join("bradian-row-example");
    let store = RowStore::open(&root)?;
    store.put(b"customer:1", b"active")?;
    assert_eq!(store.get(b"customer:1").as_deref(), Some(&b"active"[..]));
    store.checkpoint()?;
    Ok(())
}
```

```sh
cargo test -p bradian-relational --locked
```

The Rust API provides `open`, `put`, `delete`, `commit_batch`, `get`, `scan`, `len`, `last_lsn`, and `checkpoint`. A `commit_batch` accepts 1–256 operations in a single bounded WAL frame and an optional expected LSN for optimistic conflict validation; a successful batch receives one durable LSN. It accepts binary keys of 1–512 bytes and inline values up to 3,546 bytes. A `parking_lot` mutex serializes writers and reads within one process, and an OS-level exclusive advisory file lock prevents a second cooperating process from opening the same store. Rust 1.89+ is required. A `put` or `delete` return value is its synchronized WAL sequence number. A snapshot rebuilds all records onto 4 KiB physical pages and is published before WAL truncation.

**Important limitations:** This crate is **not** a SQL server, persistent catalog, general multi-statement transaction manager, MVCC engine, on-disk B+tree, multi-node lock manager, encrypted database, or fully tested cross-platform power-loss implementation. A second RowStore open for an already-locked directory is rejected. External programs must not bypass the advisory lock. Full transactions, isolation, deferred constraint enforcement, cross-platform lock qualification, key management, page allocation/free lists and filesystem failure injection remain separate mandatory engineering gates.

See [ADR 0008](../../docs/adr/0008-native-row-wal-checkpoint.md), [ADR 0007](../../docs/adr/0007-portable-physical-page.md) and the [v1.1 specification](../../docs/Bradian_Master_Architecture_SRS_v1.1.md). Rust tests are included, but automatic CI is disabled under the present repository policy.
