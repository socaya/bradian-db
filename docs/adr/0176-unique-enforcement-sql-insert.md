# ADR 0176 — Table-level `UNIQUE` at CREATE TABLE + enforcement at SqlEngine INSERT (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-21.** Partial close of
BRD-REL-002 by shipping the first SQL-layer wiring that
ADR 0101 §2 enumerated as deferred:

> **SQL layer wiring** — INSERT/UPDATE in `SqlEngine`,
> `DomainSqlEngine`, `HistorySqlEngine`, `MvccSqlEngine`,
> `DurableMvccStore`. Each engine needs its own integration
> slice that scans its row set under the write lock.

## 1. Scope

- **Parser** (`SqlEngine::execute`, CREATE TABLE path):
  accepts `UNIQUE (col[, ...])` as a table-level
  constraint clause alongside `PRIMARY KEY (...)` and
  column-definition entries. Name auto-generated as
  `uq_<col1>_<col2>_...`.
- **Catalog** already accepted `TableSpec.unique_keys`
  per ADR 0101; this ADR simply stops passing an empty
  vec from the parser. Name validity and column
  existence are verified by `Catalog::create_table`.
- **TypedTable validation**: `validate_table` no longer
  refuses tables with `unique_keys`. SQL-layer
  enforcement (below) is the authoritative gate; the
  table-store itself is schema-neutral about UNIQUE.
  `FOREIGN KEY` still refused.
- **INSERT enforcement** (`SqlEngine::execute_impl`):
  before calling `TableStore::insert`, scans the entire
  current table via `TableStore::scan()` and invokes
  `UniqueConstraintChecker::check(candidate, existing)`.
  On violation returns a new
  `SqlError::Unique(UniqueViolation)` variant that
  carries the offending constraint name and column list.
- **Honest cost**: O(N) scan per INSERT. Acceptable at
  Gate 1 scale; a disk-backed secondary index is ADR
  0127's domain + the ADR 0096 mutable-storage
  follow-up.

## 2. Out of scope

- **UPDATE path.** UPDATE's unique-enforcement is a
  separate slice that must exclude the row being
  updated from the existing-rows iterator. Deferred.
- **`DomainSqlEngine` / `HistorySqlEngine` /
  `MvccSqlEngine`.** Only `SqlEngine` in this slice.
- **Column-level `TYPE UNIQUE`.** Only table-level
  `UNIQUE (col[, ...])`. Column-level is a trivial
  parser follow-up.
- **Deferred constraints.** All checks remain
  immediate; `DEFERRABLE INITIALLY DEFERRED` grammar
  is not parsed.
- **Disk-backed secondary index.** Scan-based
  enforcement only; ADR 0127 helper remains unwired.
- **FOREIGN KEY.** Separate ADR.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/unique_enforcement_sql.rs`
(8 tests, all green):

1. `parser_accepts_unique_clause_and_first_insert_succeeds`.
2. `duplicate_unique_insert_is_rejected_with_sql_error_unique`
   — returns `SqlError::Unique { constraint: "uq_email",
   columns: ["email"] }`.
3. `distinct_unique_values_round_trip_freely`.
4. `composite_unique_rejects_only_full_tuple_duplicate` —
   `UNIQUE (first, last)`: partial matches allowed,
   full-tuple duplicate rejected.
5. `null_in_unique_column_disables_the_constraint_for_that_row`
   — SQL-standard semantics (two NULL emails coexist;
   non-NULL still enforces).
6. `table_without_unique_accepts_duplicates_freely` — no
   accidental enforcement.
7. `unique_with_no_columns_is_rejected_at_parse` —
   `UNIQUE ()` parses as a syntax / resolution error.
8. `unique_constraint_persists_after_restart` — the
   catalog durably stores the unique_keys and
   enforcement works after reopen.

Clippy clean on `-D warnings`. Existing tests continue
to compile and pass.

## 4. BRADIAN_SQL_SUBSET.md impact

Section 3 updated: CREATE TABLE grammar extended with
`[, UNIQUE (col[, ...])]`. The documented covering test
is `tests/unique_enforcement_sql.rs::parser_accepts_unique_clause_and_first_insert_succeeds`.

## 5. Roadmap impact

- BRD-REL-002 UNIQUE enforcement: `SqlEngine` INSERT
  closed. UPDATE + other engines explicitly remain
  open.
- BRD-STO-001 secondary indexes: still unimplemented;
  this slice does not create one. `UniqueConstraintChecker`
  is a scan-based primitive.
- ADR 0127 non-unique secondary index helper is
  independent and remains unwired.
