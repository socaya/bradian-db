# Master Gate 1 — column-level CHECK constraint verification

21 November 2026. ADR 0182. Minimal CHECK scope closes
the ADR 0113 §2 and ADR 0101 §2 follow-up.

## Executed result

| Executed check | Result |
|---|---|
| `cargo test --locked -p bradian-relational --test check_constraint_sql` | 7 passed, 0 failed |
| `cargo test --locked -p bradian-relational --test sql_subset_corpus rejects_join` | 1 passed (range-predicate rejection preserved) |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## Finding and action

The parser refused `CHECK (...)` in CREATE TABLE and no
schema-level predicate enforcement existed. The gap
analysis §4.1 noted "CHECK is not parsed."

Change (ADR 0182):

- **Grammar**: `col TYPE [NOT NULL] [COLLATE x] CHECK
  (<op> <literal>) [CHECK (...)]*`. Implicit LHS is the
  column being defined.
- **Operators**: `=`, `<>`, `<`, `<=`, `>`, `>=`.
- **Lexer**: `<` and `>` added to the recognized symbol
  set; WHERE / UPDATE predicate paths still only consume
  `=` so range-predicate rejection stays intact.
- **Type check at CREATE**: stored literal must match
  the column's declared type.
- **Persistence**: `Table.column_checks` and
  `TableSpec.column_checks` with
  `#[serde(default, skip_serializing_if = empty)]`.
- **Enforcement**: shared `sql::check_row(table,
  candidate)` helper called at INSERT and UPDATE by all
  four engines. Violation returns new
  `SqlError::Check { column, op, literal }`.
- **NULL**: SQL three-valued logic — NULL columns skip
  their CHECK.

Added tests
(`crates/bradian-relational/tests/check_constraint_sql.rs`,
7 total): accept/reject, UPDATE rejection, multi-clause
AND, NULL skip, all six operators, literal-type check at
CREATE, restart persistence.

## Scope limits (honest)

- **Single column, `<op> <literal>` only.** No row-level
  / multi-column CHECK, no cross-column references, no
  AND / OR boolean combinators inside one clause, no
  `IS NULL` / `IS NOT NULL`, no `IN` / `BETWEEN` /
  `LIKE`.
- **Lexer extension does not loosen WHERE.** Range
  predicates in WHERE remain rejected (ADR 0112 test
  still passes).
- **No deferred / deferrable CHECKs.** Immediate.

## Evidence

- Source:
  `crates/bradian-relational/src/model.rs` — new
  `CheckOp` enum + `column_checks` fields.
  `crates/bradian-relational/src/catalog.rs` — CHECK
  literal type validation + persistence.
  `crates/bradian-relational/src/sql.rs` — lexer `<`
  / `>`, CHECK parser clause, `check_row` helper, new
  `SqlError::Check` variant, INSERT / UPDATE wiring.
  `crates/bradian-relational/src/domain_sql.rs`,
  `crates/bradian-relational/src/history_sql.rs`,
  `crates/bradian-relational/src/mvcc_sql.rs` — shared
  `check_row` wiring at INSERT and UPDATE.
- Tests:
  `crates/bradian-relational/tests/check_constraint_sql.rs`.
- ADR: `docs/adr/0182-column-check-constraint.md`.
