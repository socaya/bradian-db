# ADR 0182 — Minimal column-level `CHECK` constraint (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-21.** Closes the ADR 0113 §2
and ADR 0101 §2 follow-up:

> **CHECK constraint parsing** — follow-up.

## 1. Scope

- **Grammar**: a column definition may carry one or more
  `CHECK (<op> <literal>)` clauses after the optional
  `NOT NULL` and `COLLATE <name>` modifiers. The LHS is
  **implicit**: it is the column being defined.
- **Operators**: `=`, `<>`, `<`, `<=`, `>`, `>=`.
- **Lexer**: `<` and `>` are added to the recognized
  symbol set so CHECK can parse comparisons. The WHERE /
  UPDATE predicate paths still consume only `=` tokens,
  so range-predicate rejection for WHERE (per ADR 0112)
  is preserved and confirmed by the existing
  `rejects_join_aggregate_and_or_and_range_and_alias`
  test.
- **Type check at CREATE**: the stored literal must match
  the column's declared type. A CHECK like `BIGINT CHECK
  (> 'abc')` fails at CREATE via `coerce` as
  `SqlError::Type`.
- **Persistence**: new
  `Table.column_checks: BTreeMap<String, Vec<(CheckOp,
  Value)>>` (`#[serde(default,
  skip_serializing_if = empty)]`). Also added as a field
  to `TableSpec` with the same serde discipline.
- **Enforcement**: every engine
  (`SqlEngine`, `DomainSqlEngine`, `HistorySqlEngine`,
  `MvccSqlEngine`) calls a shared
  `sql::check_row(table, candidate)` helper at INSERT
  and UPDATE. Violations return a new
  `SqlError::Check { column, op, literal }`.
- **NULL semantics**: SQL three-valued logic — a NULL
  value in a checked column makes the predicate UNKNOWN,
  treated as not-false, and the CHECK is skipped for
  that row.

## 2. Out of scope

- **Row-level / multi-column CHECK** (e.g. `CHECK (a <
  b)`). Only the single-column, `<op> <literal>` form.
- **`CHECK (col_ref <op> col_ref)`**. No cross-column
  references.
- **AND / OR combinators inside one `CHECK`**. Multiple
  CHECK clauses on one column are AND-joined externally;
  OR and arbitrary boolean expressions are not parsed.
- **`IS NULL` / `IS NOT NULL`**. NULL semantics apply
  implicitly; no explicit nullness predicate.
- **`NOT IN` / `IN` / `BETWEEN` / `LIKE`**. Deferred.
- **`CHECK` on composite / foreign-key relations**.
  Out of scope.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/check_constraint_sql.rs`
(7 tests, all green):

1. `check_greater_than_zero_accepts_positive_and_rejects_zero_or_negative`
   — `SqlError::Check { column: "qty", op: ">", .. }`.
2. `check_update_mutated_row_is_rejected_when_predicate_fails`
   — UPDATE mutating the row to break the predicate is
   rejected.
3. `multiple_check_clauses_on_one_column_are_all_enforced`
   — `CHECK (> 0) CHECK (< 100)` requires the value to
   land in `(0, 100)`.
4. `check_null_skips_the_predicate_per_sql_three_valued_logic`.
5. `all_six_operators_bind_correctly` — one table per
   operator; six satisfying INSERTs and six just-failing
   ones.
6. `check_literal_must_match_column_type_at_create` —
   `BIGINT CHECK (> 'abc')` fails at CREATE with
   `SqlError::Type`.
7. `check_persists_across_restart` — reopen keeps the
   constraint active.

Clippy clean on `-D warnings`. The ADR 0112 range-
rejection corpus continues to pass; adding `<`/`>` to
the lexer did not loosen the WHERE grammar.

## 4. Roadmap impact

- BRD-REL-002 CHECK: minimum scope delivered. Row-level
  / multi-column / boolean-combinator CHECKs remain
  explicitly open.
- ADR 0113 §2 "CHECK constraint parsing" follow-up
  closed at minimum scope.
- No change to BRD-STO-001 secondary-index work.

## 5. Interaction with ADRs 0176–0179

The INSERT / UPDATE enforcement order is now:

1. **CHECK** (ADR 0182) — rejects first on predicate
   violation.
2. **UNIQUE** (ADR 0176–0179) — then scans for
   duplicates on allowed values.
3. **Row persist** (`store.insert` / `store.replace`
   / `tx.insert` / `tx.replace`).

This ordering avoids an O(N) UNIQUE scan when the
candidate was never going to be accepted.
