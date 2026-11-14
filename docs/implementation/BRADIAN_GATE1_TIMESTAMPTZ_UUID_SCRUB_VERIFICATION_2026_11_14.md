# Master Gate 1 — TimestampTz/Uuid SQL-reachability scrub verification

14 November 2026. Baseline: `50f157e`.
Stable task G1-REFERENCE in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**848 Rust tests passed, 0 failed, 0 ignored**. Clippy clean. No
production code change; the engine behaviour is unchanged.

| Executed check | Result |
|---|---|
| `cargo test --workspace` | 848 passed |
| `cargo test -p bradian-relational --test sql_subset_corpus` | 36 passed (+1 new) |
| `cargo clippy -p bradian-relational --all-targets -- -D warnings` | Passed |

## Finding and action

The pre-slice reference `docs/reference/BRADIAN_SQL_SUBSET.md` §14
claimed that `TimestampTz` / `Uuid` sort behaviour was "exercised by
other suites in the crate but not yet in the single-file corpus". An
audit of `sql.rs::data_type` (the only SQL type parser) showed this
was wrong: neither `TIMESTAMPTZ` nor `UUID` is accepted in any SQL
statement on any of the three engines. The `Value::TimestampTz` /
`Value::Uuid` variants and the `select_window.rs` comparators exist
for internal engine use and future SQL work, but today they are
unreachable through SQL.

The honest action was to **scrub the §14 bullet** and prove the
unreachability with an executable test rather than silently adding
coverage that could not exist.

Changes:

- `tests/sql_subset_corpus.rs` — new
  `unsupported_data_types_in_create_table_are_rejected` test proving
  `CREATE TABLE ... TIMESTAMPTZ` and `CREATE TABLE ... UUID` are
  refused by the SQL parser.
- Reference §1 — the single-line note is rewritten to explicitly state
  the SQL unreachability and cite the new test.
- Reference §5 — a rejection row for `TIMESTAMPTZ` / `UUID` in
  `CREATE TABLE`.
- Reference §14 — the "exercised by other suites" line is replaced
  with the accurate statement that these types are intentionally not
  reachable through SQL; the `ORDER BY` row in §11 only covers
  SQL-reachable types.

## Remaining scope

- Range `ORDER BY` cases (beyond `DESC LIMIT OFFSET`) are still not in
  the single-file corpus.
- External SQL-92/99/2016 reference-suite compliance remains the open
  G1-REFERENCE acceptance bar.

See [ADR 0055](../adr/0055-sql-subset-conformance-corpus.md).
