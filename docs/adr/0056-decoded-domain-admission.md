# ADR 0056 — Decoded multi-table-domain admission (G1-MEMORY)

**Status:** Accepted for `MultiTableStore`'s decoded registered-table map.
Gate 1 remains active. This does not accept the full master G1-MEMORY exit
(global process RSS, per-engine aggregate budgets, cross-domain totals).

`MultiTableStore` rebuilds its `tables: BTreeMap<TableId, Registered>`
decoded map during `open()` by scanning every metadata record in the
underlying `RowStore`. `register_table()` grows the same map one entry at
a time. Neither path admitted entries, so a hostile source or runaway
caller could materialise the entire registered-table set without a budget
check even when the enclosing `CatalogLimits` cap was tight.

`DomainLimits { max_tables }` is added to `runtime_limits.rs` with a
default of 65_536 (the same as `CatalogLimits::max_tables`). The cap is
per-domain, not global: one process can legitimately own several shared-
WAL domains (one per database, for example) and each has its own budget.
`ResourceError::DomainLimit { resource, limit }` is distinct from storage,
corruption and catalog admission errors.

`MultiTableStore::open_with_limits(root, limits)` accepts an explicit
policy and records it on the store. The default `MultiTableStore::open`
retains its existing signature and uses `DomainLimits::default()`.
`resource_limits()` and `registered_table_count()` expose the configured
policy and current count.

Enforced invariants:

1. During `open()` replay, after each `tables.insert(...)` succeeds,
   `DomainLimits::check_tables(tables.len())` runs. A refusal returns
   `ResourceError::DomainLimit` without publishing the store and leaves
   the WAL exactly as it was.
2. During `register_table()`, the admission check runs on
   `registered.len() + 1` **before** any WAL append. A refusal leaves
   the live in-memory map, the WAL and the catalog unchanged.

Not enforced by this ADR and explicitly open:

- Decoded-state accounting across multiple domains in the same process.
- Row-value decoded state inside a transaction (bounded separately by
  staging and single-table snapshot admission, ADRs 0042/0044).
- Cross-engine totals (`MultiTableStore` + `DurableMvccStore` +
  `PersistentCatalog`).
- A global process-wide decoded-state ceiling (the §13 owner decision
  recorded in the comprehensive implementation plan).

No persistent format, wire protocol, SQL grammar or dependency change.
Existing callers see identical behaviour under the defaults; none of the
shipped tests hit the default cap.
