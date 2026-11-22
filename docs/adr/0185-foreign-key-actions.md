# ADR 0185 — FOREIGN KEY `ON DELETE` action keywords (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-21.** Follow-up to ADR 0184.
Closes the ADR 0113 §2 "FOREIGN KEY action keywords
(CASCADE / SET NULL / RESTRICT)" follow-up.

## 1. Scope

- **Parser**: optional `ON DELETE <action>` and `ON UPDATE
  <action>` clauses after `FOREIGN KEY (child_cols)
  REFERENCES <qual> (parent_cols)`.
- **Actions**: `CASCADE`, `SET NULL`, `RESTRICT`,
  `NO ACTION` (treated as `RESTRICT`; no deferral in this
  scope).
- **Catalog**: `ForeignKey`, `ForeignKeySpec` gain
  `on_delete` and `on_update` fields with
  `#[serde(default, skip_serializing_if =
  ReferentialAction::is_restrict)]` so pre-ADR-0185 bytes
  decode identically. `add_foreign_key` refuses
  `SET NULL` on a NOT NULL child column at CREATE.
- **SqlEngine DELETE**: `foreign_key_restrict` now
  branches on each FK's `on_delete`:
  - `Restrict` — existing ADR 0184 behavior.
  - `Cascade` — delete each matching child row by its
    PK; recurse into any grandchildren (SQL-standard
    transitive CASCADE). Downstream constraints still
    fire, so a RESTRICT further down blocks the entire
    chain.
  - `SetNull` — set the FK columns of each matching
    child row to NULL and re-run CHECK / FK / UNIQUE
    on the mutated row before `replace`.
- **ON UPDATE**: field preserved for a future PK-UPDATE
  path. `SqlEngine` currently rejects PK UPDATE
  unconditionally, so the field is inert today.

## 2. Out of scope

- **PK UPDATE cascading**. SqlEngine rejects PK UPDATE;
  the `on_update` field is reserved.
- **Deferrable / initially deferred FKs**. Immediate
  only.
- **`DomainSqlEngine` / `HistorySqlEngine` /
  `MvccSqlEngine` wiring**. Follow-up slice.
- **Named `CONSTRAINT fk_name FOREIGN KEY ...`**.
  Auto-generated names only (same as ADR 0184).

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/foreign_key_actions_sql.rs`
(8 tests, all green):

1. `on_delete_cascade_removes_matching_children` — two
   children matching one parent are deleted.
2. `on_delete_cascade_only_removes_matching_children` —
   children with a different parent survive.
3. `on_delete_set_null_nulls_the_fk_column` — the child
   row survives with `parent_id` becoming `NULL`.
4. `on_delete_set_null_requires_nullable_child_column_at_create`
   — catalog refuses `SET NULL` on a NOT NULL FK column.
5. `explicit_on_delete_restrict_matches_default_behavior`
   — `ON DELETE RESTRICT` == omitted clause.
6. `on_delete_no_action_is_treated_as_restrict`.
7. `cascade_is_transitive_across_fk_chain` — `a <- b <- c`
   all CASCADE; deleting `a` empties all three tables.
8. `cascade_respects_restrict_on_grandchild` — chain
   `a <- b CASCADE, b <- c RESTRICT`: deleting `a`
   fails at the grandchild RESTRICT.

The ADR 0184 suite (10 tests) continues to pass
unchanged (default `RESTRICT` preserved). Clippy clean
on `-D warnings`.

## 4. Roadmap impact

- BRD-REL-002 FK across `SqlEngine`: RESTRICT (ADR 0184)
  + CASCADE / SET NULL / NO ACTION (ADR 0185) complete.
- ADR 0113 §2 FK action keywords follow-up closed.
- Other three SQL engines remain explicitly open
  (mirror slice).
