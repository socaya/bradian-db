# Master Gate 1 — `UNIQUE` at SqlEngine INSERT verification

21 November 2026. ADR 0176. Partial BRD-REL-002 closure
for `SqlEngine` INSERT; does not claim UNIQUE for UPDATE
or the other SQL engines.

## Executed result

| Executed check | Result |
|---|---|
| `cargo test --locked -p bradian-relational --test unique_enforcement_sql` | 8 passed, 0 failed |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## Finding and action

ADR 0101 shipped the `UniqueConstraintChecker` primitive
(scan-based, SQL-standard NULL semantics, composite-key
aware) but its §2 enumerated "SQL layer wiring" — for
every engine — as deferred. The catalog has stored
`unique_keys` on tables since ADR 0009; the SQL parser
simply passed an empty vec through and `validate_table`
refused any table that declared UNIQUE.

Change (ADR 0176):

- SQL parser (CREATE TABLE path) accepts
  `UNIQUE (col[, ...])` as a table-level clause.
  Constraint name auto-generated: `uq_<col1>_<col2>_...`.
- `typed_table::validate_table` no longer refuses
  `unique_keys`; FOREIGN KEY remains refused.
- SQL INSERT path (`execute_impl` in `SqlEngine`) scans
  the current table via `TableStore::scan()` and
  invokes `UniqueConstraintChecker::check(candidate,
  existing)` before calling `insert`. On violation,
  returns a new `SqlError::Unique(UniqueViolation)`
  variant.
- BRADIAN_SQL_SUBSET.md §3 updated with the UNIQUE
  clause and its covering test.

Added tests
(`crates/bradian-relational/tests/unique_enforcement_sql.rs`,
8 total): accept + first-insert; duplicate-reject with
`SqlError::Unique { constraint, columns }`; distinct-
values round-trip; composite partial vs. full-tuple;
NULL disables (two NULLs coexist); table without
UNIQUE accepts duplicates; empty UNIQUE parse-reject;
constraint persists across restart.

## Scope limits (honest)

- **O(N) scan per INSERT.** No secondary index. ADR
  0127's `NonUniqueIndex` helper is independent and
  remains unwired.
- **INSERT path only in `SqlEngine`.** UPDATE path and
  `DomainSqlEngine` / `HistorySqlEngine` /
  `MvccSqlEngine` wiring remain out of scope.
- **No column-level `TYPE UNIQUE`.** Table-level
  `UNIQUE (col[, ...])` only.
- **No deferred / deferrable constraints.** Immediate
  check.
- **No FOREIGN KEY.** Remains refused in
  `validate_table`.

## Evidence

- Source:
  `crates/bradian-relational/src/sql.rs` — CREATE TABLE
  UNIQUE parse; INSERT enforcement call; new
  `SqlError::Unique` variant.
  `crates/bradian-relational/src/typed_table.rs` —
  `validate_table` relaxed on UNIQUE.
- Tests:
  `crates/bradian-relational/tests/unique_enforcement_sql.rs`.
- ADR: `docs/adr/0176-unique-enforcement-sql-insert.md`.
- Reference doc: `docs/reference/BRADIAN_SQL_SUBSET.md`
  §3 updated.
