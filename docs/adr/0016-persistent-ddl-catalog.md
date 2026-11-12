# ADR 0016 — Durable relational catalog through validated DDL WAL replay

**Product:** Bradian Database Systems  
**Normative specification:** Master Architecture & SRS v1.1  
**Status:** Operational Rust source and integration tests committed to `main`; manual compilation and crash qualification outstanding.  
**Dependencies:** [ADR 0006 — catalog](0006-generic-relational-catalog.md), [ADR 0008 — WAL/checkpoints](0008-native-row-wal-checkpoint.md), [ADR 0014 — checkpoint generation](0014-atomic-checkpoint-manifest.md).

## Problem

The original `Catalog` validated database, schema, table, primary/unique key and foreign-key definitions but held all metadata in memory. Restart discarded object identities, relationships and dependency rules. A database management system cannot safely attach persisted typed rows to a regenerated or differently allocated table identity.

A durable catalog needs atomic metadata publication, stable monotonic IDs, ordered replay of validated operations and corruption rejection. It must remain independent of DHIS2 and avoid a separate validator with different semantics.

## Implemented decision

Introduce `bradian-relational::persistent_catalog::PersistentCatalog`. Each instance exclusively owns a native `RowStore` directory (including the existing operating-system file lock) and guards the in-memory `Catalog` with a `parking_lot::Mutex`.

The journal starts with the version marker `bradian.catalog.version = BRADIAN-CATALOG-DDL-V1` at WAL LSN 1. Every accepted DDL command consumes exactly one subsequent LSN, stored under the ordered ASCII key `ddl/<16-hex-digit-lsn>`. The value is a JSON-encoded, versioned DDL event containing the operation and expected allocated object identity (where applicable). The WAL itself is framed, CRC checked and synchronized using the existing `BRWL` record mechanism; optional BRPG checkpoints and the `BRCP` manifest are provided by the underlying store.

The supported operations are:

| Operation | Effect |
|---|---|
| `create_database(name)` | Validate/canonicalize identifier; allocate durable monotonic DatabaseId |
| `create_schema(database, name)` | Validate database parent and scoped uniqueness; allocate SchemaId |
| `create_table(schema, spec)` | Validate columns, types, keys and uniqueness; allocate TableId |
| `add_foreign_key(table, spec)` | Validate source and target columns, types, unique target and same-database rule |
| `drop_table(table)` | Enforce RESTRICT on dependent foreign-key references |
| `drop_schema(schema)` | Enforce RESTRICT while tables remain |
| `drop_database(database)` | Enforce RESTRICT while schemas remain |
| `snapshot()` | Return a clone of the visible committed catalog |
| `checkpoint()` | Publish catalog journal as BRPG snapshot and derived B+tree |

## Atomic mutation protocol

1. Hold the catalog mutex for the entire DDL operation.
2. Clone the committed `Catalog`. Apply the requested operation to the clone, using **the original Catalog validators**. Any duplicate, missing parent, invalid constraint or dependent-object violation fails immediately with no WAL append.
3. Serialize a versioned event containing the operation and the identity assigned by that validated clone. The native row store enforces its inline record bound; an oversized operation is rejected before WAL mutation.
4. Append the complete event under the next sequential LSN through `RowStore::put`, synchronizing the CRC-protected WAL frame before returning success.
5. Only after WAL sync, publish the tentative cloned `Catalog` as the visible in-memory metadata state.

The journal is the durable source of truth for catalog metadata. One metadata operation is individually atomic; this does **not** provide arbitrary multi-operation SQL DDL transactions, transactional table-data deletion or schema evolution of an existing typed row store.

## Recovery and corruption handling

On opening a catalog, acquire the store lock and use the underlying BRPG checkpoint and WAL recovery. A new empty store creates the version marker. An existing store must contain the exact supported marker; an incompatible or missing marker is a hard error.

Read journal rows in key order. Enforce exactly one event per expected LSN, from 2 through the native store's latest acknowledged LSN. For each event, decode its versioned JSON representation, re-apply the operation through the original Catalog implementation, and compare its allocated identity to the expected recorded identity. An invalid command, duplicate, broken dependency, missing event, unknown key namespace, bad event version, bad identity, unsupported catalog marker or inconsistent LSN is a corruption error. **Never silently skip a malformed DDL event.**

Since the same sequence of successful creates and drops is replayed from the beginning, monotonically allocated object IDs are stable even when earlier objects are dropped. Existing reader snapshots are clones and do not change when a later DDL event commits.

## Safety boundaries

- This is an **append-only local metadata event journal**. The system stores one event for every successful DDL operation and replays it on startup; metadata compaction and bounded restart cost require a separately designed generation protocol.
- The current RowStore inline value bound restricts the size of any one DDL command. Large schema definitions need overflow records or a versioned multi-key atomic transaction format before being supported.
- Secondary UNIQUE and FOREIGN KEY metadata persist, but row-level enforcement remains unavailable in the typed table engine. That engine refuses unsupported declared constraints rather than pretending they are enforced.
- The persistent catalog and independently persisted user tables **are not in the same atomic transaction**. Dropping a catalog table is metadata-only; it does not remove or synchronize the underlying typed row-store directory. Production DDL requires a shared transactional storage/catalog protocol, safe lifecycle states and migration semantics.
- The catalog is still an in-memory snapshot between accepted changes. Concurrent readers obtain owned snapshots, and concurrent writers are serialized through the catalog mutex.
- No SQL parser, transactional ALTER TABLE, multi-statement DDL rollback, MVCC, cluster consensus, encryption, fine-grained role authorization or signed journal is provided.
- CRC32 and typed replay verify accidental corruption and structural consistency, not cryptographic authenticity.
- The G10 local row-store backup infrastructure exists but does not yet automatically coordinate **one consistent backup of catalog plus every referenced table**. Database-wide backup and restore are separate deliverables.

## Integration and acceptance tests

`tests/persistent_catalog.rs` exercises restart with stable database/schema/table IDs, DDL failure without LSN advancement, FK/drop dependency replay, non-reuse of IDs after drops, WAL writes after catalog checkpoints, serialized concurrent DDL, exclusive ownership, malformed event/version marker detection, unknown key detection, overlarge DDL rejection and immutable reader snapshots.

Manual checks are required:

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

The source and tests are committed; **no passing test result is claimed**. Automatic CI remains disabled under the repository owner's direction.
