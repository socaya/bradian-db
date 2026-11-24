# ADR 0277 — In-tree SQLLogicTest-compatible runner

## Status

Accepted.

## Context

G1-REFERENCE's final tail was "external SQL-92/99/2016
conformance suite integration." Running the full external
suites requires infrastructure (license, matrix, upkeep),
but the IN-TREE runner is code we own. Shipping it now
unblocks progressively importing cases from the external
suites later.

## Decision

- New `tests/sql_conformance_runner.rs` reads `.test` files
  from `tests/sql_conformance_cases/`. Supports a subset of
  the SQLLogicTest dialect:
  - `# comment`
  - `statement ok` — SQL must succeed.
  - `statement error` — SQL must fail (any `SqlError`).
  - `query I|T` — SELECT followed by `----` and expected
    rows. One row per line, cells space-separated.
- Each `.test` file runs against a fresh `SqlEngine` in a
  `tempdir`.
- Initial corpus (6 files, ~30 directives):
  - `01_basic_select.test` — CRUD + predicate.
  - `02_arithmetic.test` — `+ * /` with divide-by-zero.
  - `03_null_handling.test` — IS NULL / IS NOT NULL.
  - `04_ddl_cascade.test` — plain DROP rejects on UK/FK
    dependents; CASCADE succeeds.
  - `05_set_ops.test` — INTERSECT, EXCEPT.
  - `06_exists_subquery.test` — EXISTS non-empty / empty.

## Consequences

- The runner is small and self-contained. A single
  integration test (`run_sql_conformance_cases`) iterates
  every `.test` file; new cases are drop-in additions.
- Adding external-suite .test files later: identify the
  SQLLogicTest-compatible subset, copy or translate into
  `tests/sql_conformance_cases/`, re-run. No code changes
  for each additional case.
- Full external-suite compliance (SQL-92/99/2016 complete
  coverage) remains a continuous process rather than a
  single deferred infrastructure gate.
- 6 conformance files cover basic SELECT, arithmetic, NULL
  predicates, DDL CASCADE, set operators, EXISTS. Every
  file passes.
