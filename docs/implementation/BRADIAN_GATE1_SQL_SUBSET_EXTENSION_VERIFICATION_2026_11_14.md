# Master Gate 1 — SQL subset corpus extension verification

14 November 2026. Baseline: `77f6bf7`.
Stable task G1-REFERENCE in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**834 Rust tests passed, 0 failed, 0 ignored**. Clippy clean on
`bradian-relational --all-targets`. CI remains manual-only. No persistent
format, wire protocol or SQL grammar change.

| Executed check | Result |
|---|---|
| `cargo test --workspace` | 834 passed, 0 failed |
| `cargo test -p bradian-relational --test sql_subset_corpus` | 33 passed (+7 new) |
| `cargo clippy -p bradian-relational --all-targets -- -D warnings` | Passed |

## Implemented and exercised

Seven new tests extend the corpus past the Slice 5 baseline:

- `describe_table_as_of_returns_schema_rows` — DESCRIBE TABLE ... AS OF.
- `describe_without_as_of_is_rejected` — the counterpart rejection.
- `release_savepoint_confirms_prior_work_and_allows_commit` — SAVEPOINT
  + RELEASE + COMMIT round-trip.
- `insert_default_uses_the_column_default_at_the_target_schema` — INSERT
  INTO ... VALUES (..., DEFAULT) with an ALTER-added default.
- `update_expression_boundary_at_32_nested_parentheses_is_accepted` — the
  published 32-depth nesting boundary.
- `update_expression_rejects_nesting_beyond_published_depth` — depth 33
  refused.
- `update_expression_rejects_arithmetic_on_text_target` — type mismatch
  at the UPDATE target column.

`docs/reference/BRADIAN_SQL_SUBSET.md` §8 and §9 add the new rows with
test citations; §14 is tightened to remove items now covered.

## Remaining scope

- The 128-node UPDATE expression boundary (as distinct from the 32-depth
  boundary) remains unexercised by the single-file corpus.
- Range-ORDER BY cases and `TimestampTz`/`Uuid` sort behaviour are
  exercised by other suites in the crate but not by the corpus.
- External SQL-92/99/2016 reference-suite compliance remains the open
  acceptance bar.

See [ADR 0055](../adr/0055-sql-subset-conformance-corpus.md).
