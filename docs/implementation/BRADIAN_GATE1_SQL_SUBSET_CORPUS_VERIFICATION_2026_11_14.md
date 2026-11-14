# Master Gate 1 — Documented SQL subset corpus verification

14 November 2026. Baseline: `3dcdd23`.
Stable task G1-REFERENCE in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**823 Rust tests passed, 0 failed, 0 ignored**. Workspace compilation and
Clippy (`-p bradian-relational --all-targets -- -D warnings`) passed.
`scripts/verify_roadmap.py` passed. CI remains manual-only. No dependency,
lockfile, persistent format, SQL grammar or default isolation changed.

Environment: Darwin 25.2.0 x86_64, Rust/Cargo stable.
The checkout was restored from published `main` before implementation.

| Executed check | Result |
|---|---|
| `cargo check --workspace` | Passed |
| `cargo test --workspace` | 823 passed, 0 failed, 0 ignored |
| `cargo test -p bradian-relational --test sql_subset_corpus` | 26 passed |
| `cargo clippy -p bradian-relational --all-targets -- -D warnings` | Passed |
| `python3 scripts/verify_roadmap.py` | 60 unique requirements, fixed sequence and evidence links |

## Implemented and exercised

A new reader-facing reference, `docs/reference/BRADIAN_SQL_SUBSET.md`,
enumerates the supported SQL surface across the three engines. Every claim
cites exactly one passing test in
`crates/bradian-relational/tests/sql_subset_corpus.rs`; a claim with no
cited test is explicitly outside the corpus.

The corpus adds **26 integration tests** across these categories:

- **Literals** — positive/negative integer, text (empty, escaped single
  quote, UTF-8), boolean, NULL (case insensitivity enforced).
- **Data types** — INT, BIGINT, VARCHAR(n), DECIMAL(p,s) round-trip
  including observed `Value` representation and scale.
- **DDL + DML (SqlEngine)** — `CREATE DATABASE/SCHEMA/TABLE`, `INSERT`
  (reordered columns), `UPDATE`, `DELETE`, `SELECT *`.
- **Common rejections** — SQL comments, multi-statement execute, partial
  PK predicate, JOIN, aggregates, `OR`, range `>`, column aliases,
  `DISTINCT`, `UNION`.
- **SqlEngine sessions** — `BEGIN`/`COMMIT` publishing one WAL batch;
  `ROLLBACK` discarding staged changes.
- **DomainSqlEngine sessions** — cross-table `BEGIN`+`COMMIT` atomicity;
  cross-table `ROLLBACK`.
- **HistorySqlEngine sessions** — `BEGIN` + `COMMIT`; `BEGIN TRANSACTION
  WITH READ VALIDATION`; `SAVEPOINT` + `ROLLBACK TO` keeping pre-marker
  work.
- **UPDATE expressions** — `col + literal` on BIGINT; parentheses override
  precedence.
- **Ordered queries** — `ORDER BY ... DESC LIMIT n OFFSET m`; oversized
  `LIMIT` refusal.
- **AS OF** — historical read returns rows at the captured LSN; `AS OF`
  refused inside `WITH READ VALIDATION`.
- **Schema evolution** — ALTER ADD COLUMN NULL succeeds after
  `enable_epoch_row_writes()`; refused without opt-in on a migrated G23
  history domain; `DROP COLUMN`/`RENAME`/type change always refused.

Each test exercises only the public engine API. Three initial claims that
seeded the reference were corrected after test execution surfaced the real
engine behavior:

1. `ALTER` on a migrated G23 history domain requires explicit
   `enable_epoch_row_writes()` opt-in, not just a `HistorySqlEngine`.
2. Statement errors inside `WITH READ VALIDATION` and oversized `LIMIT`
   abort the enclosing session; a trailing `ROLLBACK` in the same closure
   would itself fail with `ROLLBACK without BEGIN`.
3. `Value` variants are flat (`Value::Int32`, `Value::Int64`,
   `Value::String`, `Value::Decimal(i128)`) rather than structured; the
   reference describes the actual representation.

## Remaining scope

The corpus is deliberately a single-file positive/negative reference, not
an external SQL conformance suite. Open items (all explicitly listed in
`BRADIAN_SQL_SUBSET.md` §14):

- External SQL-92 / SQL-99 / SQL:2016 reference-suite compliance.
- Full documented isolation-level qualification (snapshot isolation and
  opt-in read validation are the only shipped modes).
- Client wire protocol / driver behaviour.
- Windows / macOS installer, lock lifecycle and recovery runs.
- Rows not yet in the single-file corpus: `DESCRIBE TABLE AS OF`,
  `RELEASE SAVEPOINT`, `INSERT ... DEFAULT`, range ORDER BY cases,
  `TimestampTz` / `Uuid` sort comparison, and the full 128-node /
  32-nesting-depth UPDATE expression boundary (these are exercised by
  other suites in the crate but not yet in the corpus file).

See [ADR 0055](../adr/0055-sql-subset-conformance-corpus.md),
[`BRADIAN_SQL_SUBSET.md`](../reference/BRADIAN_SQL_SUBSET.md) and the
[fixed roadmap](BRADIAN_MASTER_ROADMAP.md).
