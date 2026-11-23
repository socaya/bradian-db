# ADR 0241 — ALTER COLUMN SET NOT NULL [BACKFILL lit]

## Status

Accepted.

## Context

Master-SRS P0-1 lists broader schema evolution as a Gate 1
blocker. ALTER already supports RENAME / DROP COLUMN / SET
DEFAULT / DROP DEFAULT / TYPE widen. The visible hole was
SET NOT NULL — strengthening a column's nullability. The
historical `plan_schema_projection` enforces monotone-relaxation
only, so strengthening needs a separate catalog path that
pairs a direct metadata flip with a row-level validation
before commit.

## Decision

- `Catalog::strengthen_column_not_null(id, name)` flips the
  column's `nullable` bit from true → false. Rejects
  already-not-null or unknown columns. Row-level NULL
  validation is the caller's responsibility.
- `Command::StrengthenColumnNotNull { table, column }` and
  `PersistentCatalog::strengthen_column_not_null` surface the
  mutation through the catalog journal.
- `TableStore::rewrite_schema_pin(new_table)` rewrites the
  META_KEY schema pin on disk and updates the in-memory
  `Table` + `pk_positions`. Caller must guarantee every stored
  row still validates under the new schema.
- SQL grammar:
  `ALTER TABLE <name> ALTER COLUMN <c> SET NOT NULL [BACKFILL <literal>]`.
  The base `SqlEngine` does:
  1. Resolve the column; reject if already NOT NULL.
  2. Scan the TableStore.
  3. If any row carries NULL and no BACKFILL was supplied,
     reject with `SqlError::Type`.
  4. If BACKFILL was supplied, coerce the literal against the
     column type (rejects type mismatches) and `store.replace`
     each NULL-bearing row.
  5. Call `PersistentCatalog::strengthen_column_not_null`.
  6. `store.rewrite_schema_pin` so the next open sees the
     strengthened schema.

## Consequences

- Scoped to the base `SqlEngine` (single-table exclusive
  ownership). Transactional engines retain the monotone-
  relaxation invariant until a coordinated DDL path lands.
- Backfill is a one-shot replacement with a literal; expression
  backfill (default expression at the column level) is a
  future ADR.
- No concurrent-reader fencing yet — this is the exclusive-
  ownership mode. Coordinated online SET NOT NULL is P0-2.
- 7 cases in `tests/alter_set_not_null.rs` cover success when
  no NULLs, rejection when NULLs exist, backfill replaces
  NULLs, empty-table, already-not-null rejection, unknown-
  column rejection, backfill type-mismatch rejection.
