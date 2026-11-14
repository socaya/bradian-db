# ADR 0055 — Documented SQL subset conformance corpus (G1-REFERENCE)

**Status:** Accepted as the published positive-and-negative conformance
surface for Bradian's three native SQL engines. Gate 1 remains active.
This does not accept the full master G1-REFERENCE exit (external SQL
reference-suite compliance, full documented isolation qualification,
Windows/macOS installer/lock tests).

The Bradian kernel ships three distinct SQL engines:

- `SqlEngine` — legacy single-table session (G12–G16).
- `DomainSqlEngine` — shared-WAL multi-table session (G15–G18).
- `HistorySqlEngine` — durable history, expression UPDATE, ordered SELECT,
  savepoints, AS OF, DESCRIBE (G21+).

Each engine implements a different strict subset. Past verification reports
cite subsets by feature but no single artifact exists that enumerates every
supported statement with a proving test. Downstream consumers cannot see at
a glance whether a given statement works, on which engine, and with what
rejection behavior when it does not.

This ADR adopts a **conformance corpus**: a reader-facing reference
document (`docs/reference/BRADIAN_SQL_SUBSET.md`) backed by an executable
test file (`crates/bradian-relational/tests/sql_subset_corpus.rs`). Every
row in the reference cites the owning test by name; every test proves
exactly one claim with exact expected output (positive) or exact rejection
semantics (negative). Tests use the three public engine APIs — no engine
internals are reached.

Scope:

- **Positive corpus.** Each supported statement form on each engine.
  Includes literal syntax, data types, PK predicates, transaction
  statements, savepoints, AS OF, ORDER BY / LIMIT / OFFSET, UPDATE
  expressions, DESCRIBE, cross-table transactions.
- **Negative corpus.** Each explicitly rejected form. Includes comments,
  joins, aggregates, DISTINCT, UNION, OR/range predicates, partial-PK
  predicates, nested BEGIN, cross-table transactions on the engines that
  forbid them, statement-error recovery, oversized identifiers.

Non-goals:

- Does not claim SQL-92/SQL-99/SQL:2016 conformance.
- Does not claim serializable isolation; ordinary BEGIN remains snapshot,
  BEGIN WITH READ VALIDATION remains opt-in bounded (ADR 0037).
- Does not replace a published external SQL reference suite — the
  outstanding G1-REFERENCE acceptance bar.
- Does not add new SQL features or change rejection messages. The corpus
  captures what currently works and currently fails.
- Does not promise behavior across platforms beyond what the existing
  workspace tests already exercise.

The corpus is additive. New SQL work (joins, aggregates, broader schema
evolution) extends the corpus with new rows and new tests, never silently
reinterprets an existing claim. A rejection message change must update the
corpus test and reference row in the same commit.

No production code, persistent format, wire protocol or dependency changes.
