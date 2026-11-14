# Persistent catalog admission

This Master Gate 1 / G1-MEMORY policy bounds the decoded in-memory state
that `PersistentCatalog` reconstructs from its append-only DDL journal at
open, and the serialized bytes of a single DDL event. The journal itself
remains append-only until a versioned catalog compactor is designed.

| Budget | Default | Accounted state |
|---|---|---|
| Databases | 1024 | Every live database in the catalog |
| Schemas | 4096 | Every live schema |
| Tables | 65_536 | Every live table |
| Pending revisions | 1024 | Every durable G33+ proposal awaiting activation or cancellation |
| DDL event bytes | 64 KiB | Serialized bytes of one journaled `Event`, independently checked on write and replay |

These are **per-catalog** counts; one process can own several catalogs
(one per database directory). `ResourceError::CatalogLimit { resource,
limit }` is distinct from catalog semantic errors, storage errors, JSON
encoding errors and corruption.

## Native configuration and accounting

```rust
use bradian_relational::persistent_catalog::PersistentCatalog;
use bradian_relational::runtime_limits::CatalogLimits;

let policy = CatalogLimits {
    max_databases: 4,
    max_schemas: 16,
    max_tables: 128,
    max_revisions: 32,
    max_event_bytes: 16 * 1024,
};
let catalog = PersistentCatalog::open_with_limits("./catalog", policy)?;
let usage = catalog.resource_usage();
let limits = catalog.resource_limits();
```

`resource_usage()` reports current `(databases, schemas, tables,
revisions)`. `resource_limits()` reports the selected policy. The default
`PersistentCatalog::open` uses `CatalogLimits::default()`.

## Refusal and recovery

`open_with_limits` admits the serialized bytes of each journaled event
before decoding and re-checks the decoded counts after each
`Event::apply`. A refusal returns `ResourceError::CatalogLimit` and the
WAL journal is byte-identical to pre-open; the catalog is not
constructed. A later reopen with a sufficient policy still sees every
previously committed DDL event.

`commit()` (the internal entry point behind `create_database`,
`create_schema`, `create_table`, `add_foreign_key`, `drop_*` and all
`propose_*` / `activate_table_revision` / `cancel_table_revision` calls)
runs the admission check after the replay validator updates the
tentative catalog and revisions map, and **before** any WAL append. A
refusal leaves the live catalog, revisions and journal exactly as they
were. The journal is append-only — a successful DDL event cannot be
"undone" by a later catalog reopen under tighter limits.

## Example tightening and trade-offs

For a development configuration restricted to one database with a few
schemas:

```rust
let strict = CatalogLimits {
    max_databases: 1,
    max_schemas: 4,
    max_tables: 64,
    ..CatalogLimits::default()
};
```

For an archival catalog that must accept historical journals but refuse
new growth, open under the current counts:

```rust
let open_catalog = PersistentCatalog::open(&path)?;
let current = open_catalog.resource_usage();
drop(open_catalog);
let frozen = CatalogLimits {
    max_databases: current.databases,
    max_schemas: current.schemas,
    max_tables: current.tables,
    max_revisions: current.revisions,
    max_event_bytes: CatalogLimits::default().max_event_bytes,
};
let frozen_catalog = PersistentCatalog::open_with_limits(&path, frozen)?;
// Any CREATE now refuses with CatalogLimit.
```

Policies are not persisted; they bound live decoded state for the
process that opens the catalog.

## Remaining scope

- Per-table column, FK and unique-key counts beyond the existing
  `RowStore` inline-record limit on serialized event bytes.
- Decoded catalog state across multiple domains in the same process.
- A versioned catalog compactor; the journal is still append-only, so
  committed DDL cannot be forgotten by simply dropping objects.
- A global process-wide decoded-state ceiling — §13 owner-decision item
  in the [comprehensive implementation plan](../implementation/BRADIAN_COMPREHENSIVE_IMPLEMENTATION_PLAN.md).

See [ADR 0054](../adr/0054-decoded-catalog-admission.md) and
[executed verification](../implementation/BRADIAN_GATE1_CATALOG_LIMITS_VERIFICATION_2026_11_14.md).
