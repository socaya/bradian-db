# ADR 0184 — FOREIGN KEY RESTRICT enforcement in `SqlEngine` (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-21.** Lands the ADR 0101 §2
"FOREIGN KEY enforcement — a separate ADR" prerequisite.
Only `SqlEngine` here; `DomainSqlEngine`,
`HistorySqlEngine`, `MvccSqlEngine` wiring is a follow-up,
and ON DELETE / ON UPDATE action keywords are ADR 0185.

## 1. Scope

- **Parser**: table-level clause
  `FOREIGN KEY (child_cols) REFERENCES <qual_parent>
  (parent_cols)` in CREATE TABLE. No `ON DELETE` /
  `ON UPDATE` clauses yet (ADR 0185).
- **Catalog wiring**: parser calls
  `catalog.create_table` to allocate the child, then
  resolves each parent name via the catalog snapshot and
  calls the existing `catalog.add_foreign_key(child_id,
  spec)`. That path already validates arity, type
  match, target-is-PK-or-non-null-UNIQUE, and
  cross-database refusal.
- **TypedTable**: `validate_table` no longer refuses
  `foreign_keys` — enforcement is done at the SQL
  engine layer, matching the UNIQUE pattern (ADR 0176).
- **INSERT**: `foreign_key_check(engine, &table, &row)`
  scans each FK's parent table and ensures a matching
  row exists. A NULL in any FK column disables that
  FK for the row (SQL-standard; matches the UNIQUE
  NULL rule).
- **UPDATE**: `foreign_key_check` runs on the mutated
  row to cover FK-column changes.
- **DELETE**: `foreign_key_restrict(engine, &parent,
  &pk)` scans every child table whose FK points at
  this parent's PK and refuses the DELETE if any
  child row matches. This is the SQL **RESTRICT**
  default.
- **New `SqlError::ForeignKey { constraint, reason }`**
  with `reason` ∈ `"missing-parent"` (INSERT /
  UPDATE) / `"restrict"` (DELETE).

Constraint names are auto-generated `fk_<table>_<i>`.

## 2. Out of scope

- **CASCADE / SET NULL / NO ACTION action keywords**
  — parser-only extension deferred to ADR 0185.
- **PK UPDATE cascading** — PK UPDATE is already
  rejected by `SqlEngine`; nothing to do.
- **DomainSqlEngine, HistorySqlEngine, MvccSqlEngine
  wiring** — separate follow-up slices mirroring the
  pattern.
- **Deferrable / initially deferred FKs** — not parsed.
- **Named explicit FK constraint names
  (`CONSTRAINT fk_name FOREIGN KEY ...`)** — auto-
  generated only.
- **Cross-database FKs** — already refused by the
  catalog.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/foreign_key_sql.rs`
(10 tests, all green):

1. `child_insert_with_existing_parent_succeeds`.
2. `child_insert_without_parent_is_rejected` —
   `SqlError::ForeignKey { reason: "missing-parent" }`.
3. `child_update_fk_column_to_missing_parent_is_rejected`.
4. `parent_delete_is_restricted_when_children_exist` —
   `SqlError::ForeignKey { reason: "restrict" }`.
5. `parent_delete_after_children_removed_succeeds` —
   RESTRICT clears once no children match.
6. `nullable_fk_column_null_disables_check` — NULL FK
   value skips the FK for that row.
7. `composite_fk_rejects_partial_match` — tuple
   `(1, 4)` is rejected even though `1` and `4`
   appear separately in parent rows.
8. `foreign_key_persists_across_restart` — both
   RESTRICT and missing-parent still fire after a
   catalog reopen.
9. `foreign_key_referencing_missing_parent_name_is_rejected_at_create`
   — unknown parent qualified name fails at CREATE
   with `SqlError::Resolution`.
10. `foreign_key_arity_mismatch_is_rejected_at_create`
    — arity differs → `SqlError::Catalog` via
    `add_foreign_key`'s existing validation.

Clippy clean on `-D warnings`.

## 4. Enforcement order at INSERT / UPDATE

The enforcement order established by ADRs 0176–0182 is
now:

1. CHECK (ADR 0182) — cheapest, column-local.
2. FK missing-parent (ADR 0184) — scans parent
   table(s) per FK.
3. UNIQUE (ADRs 0176–0179) — scans this table.
4. Row persist (`store.insert` / `store.replace`).

DELETE order:

1. Existence check (row must exist).
2. FK RESTRICT (ADR 0184) — scans every referencing
   child table.
3. Delete.

## 5. Roadmap impact

- ADR 0101 §2 FK-enforcement follow-up delivered at
  `SqlEngine` scope. The other three SQL engines
  remain explicitly open.
- ADR 0113 §2 FK action keywords are the next slice.
- BRD-REL-002 progress: FK RESTRICT closed for
  `SqlEngine`.
