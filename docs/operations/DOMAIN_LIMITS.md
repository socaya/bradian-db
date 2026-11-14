# Multi-table domain admission

This Master Gate 1 / G1-MEMORY policy bounds the decoded
registered-table map that `MultiTableStore` rebuilds from its shared-WAL
metadata at open, and the live map that `register_table` grows. This
complements, and is independent of, the `PersistentCatalog` cap on total
table count.

| Budget | Default | Accounted state |
|---|---|---|
| Registered tables | 65_536 | Every pinned schema in this domain's `tables: BTreeMap<TableId, Registered>` |

The cap is **per-domain**, not global. One process can legitimately own
several shared-WAL domains (for example, one per database) and each has
its own budget. `ResourceError::DomainLimit { resource, limit }` is
distinct from storage, corruption and catalog admission errors.

## Native configuration and accounting

```rust
use bradian_relational::transaction_domain::MultiTableStore;
use bradian_relational::runtime_limits::DomainLimits;

let policy = DomainLimits { max_tables: 64 };
let domain = MultiTableStore::open_with_limits("./shared-domain", policy)?;
assert_eq!(domain.resource_limits().max_tables, 64);
assert!(domain.registered_table_count() <= 64);
```

`MultiTableStore::open` keeps its prior signature and uses
`DomainLimits::default()`. `resource_limits()` and
`registered_table_count()` expose the configured policy and current
count. `DomainSqlEngine` wraps `MultiTableStore` and inherits these
budgets automatically; a dedicated `open_with_limits` wrapper on the SQL
side is not yet exposed.

## Refusal and recovery

`open_with_limits` admits each row from the metadata iterator after it
is inserted into the tentative map. A refusal returns
`ResourceError::DomainLimit`, does not construct the store and leaves
the WAL byte-identical. A later reopen with a relaxed policy still sees
every previously registered table.

`register_table()` admits `registered.len() + 1` **before** any WAL
append. A refusal leaves the live in-memory map, the WAL and the
catalog exactly as they were. Idempotent re-registration of a table
whose schema already matches does not charge additional capacity.

## Example tightening

For a tenant-scoped domain that must not grow past a known table set:

```rust
let strict = DomainLimits { max_tables: 8 };
let domain = MultiTableStore::open_with_limits(&path, strict)?;
// Attempting to register a ninth table refuses with DomainLimit.
```

Interaction with upstream catalog admission: `CatalogLimits.max_tables`
bounds the **total** tables across every catalog; `DomainLimits.max_tables`
bounds a **single** shared-WAL domain. A domain cap lower than the
catalog cap is the usual configuration.

## Remaining scope

- Cross-engine decoded-state totals combining `MultiTableStore`,
  `DurableMvccStore` and `PersistentCatalog`.
- Row-value decoded state inside a transaction is bounded separately
  by staging and single-table snapshot admission (ADRs 0042 and 0044).
- A global process-wide decoded-state ceiling — §13 owner-decision
  item in the [comprehensive implementation plan](../implementation/BRADIAN_COMPREHENSIVE_IMPLEMENTATION_PLAN.md).

See [ADR 0056](../adr/0056-decoded-domain-admission.md) and
[executed verification](../implementation/BRADIAN_GATE1_DOMAIN_LIMITS_VERIFICATION_2026_11_14.md).
