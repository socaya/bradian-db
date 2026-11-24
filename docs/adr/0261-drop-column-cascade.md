# ADR 0261 — DROP COLUMN CASCADE

## Status

Accepted.

## Context

The dependency planner (ADR 0126) reports edges that block a
plain `DROP COLUMN`. Users had no way to express
"drop this column and everything that depends on it" — they
had to walk the planner themselves, drop each FK/UK by name,
and then drop the column. CASCADE is the standard shortcut.

## Decision

- New `Catalog::drop_foreign_key(id, name)` /
  `drop_unique_key(id, name)` for individual constraint
  removal.
- New `Catalog::drop_column_cascade(id, column)` that:
  1. Computes the dependency closure (local UKs, local FKs,
     incoming FKs from other tables).
  2. Drops each in order.
  3. Drops the column.
- Primary-key columns are **not** CASCADE-able — the only way
  out is `DROP TABLE`. Returns `CatalogError::DependentObjects`
  so the caller gets a clear message.
- Last-column drop still rejected (empty-table invariant).
- `PersistentCatalog` journals `DropColumnCascade`,
  `DropForeignKey`, `DropUniqueKey` so reopens replay
  deterministically.
- SQL grammar: `ALTER TABLE t DROP COLUMN c CASCADE`. The old
  `DROP COLUMN c` without CASCADE preserves its strict
  behavior.

## Consequences

- CASCADE touches incoming FKs in other tables — this is a
  cross-table edit. The dependents list is computed once,
  then each drop is journaled separately. A failed CASCADE
  may leave partial state (some FKs dropped but the column
  remains); callers must retry CASCADE to finish. A
  transactional CASCADE spanning multiple commits is a
  future slice.
- `DROP FOREIGN KEY <name>` and `DROP UNIQUE <name>` are not
  yet surfaced as SQL grammar — the catalog methods are
  reachable through the `PersistentCatalog` public API for
  programmatic use. SQL grammar is a short follow-up.
- 6 cases in `tests/drop_column_cascade.rs` cover:
  plain drop rejected on UK column; CASCADE drops the column
  plus the incoming FK; CASCADE on PK rejected; CASCADE on
  last column rejected; CASCADE persists across reopen;
  directly asserts incoming FK removal.
