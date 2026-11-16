# ADR 0101 — Secondary UNIQUE constraint utility (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16 as a correctness primitive.**

## 1. Honest scope

Catalog already accepts `unique_keys` at CREATE TABLE
(`catalog.rs:159`). ADR 0009 noted that enforcement during
mutation was deliberately deferred. This ADR ships the
**enforcement primitive** that a future SQL INSERT/UPDATE path
can call before committing a candidate row:

- `bradian_relational::unique_check::UniqueConstraintChecker`
- `UniqueConstraintChecker::check(candidate, existing_iter)
  -> Result<(), UniqueViolation>`
- SQL-standard NULL treatment: NULL in a unique-key column
  disables the constraint for that row.
- Composite unique keys.

## 2. Out of scope

- **SQL layer wiring** — INSERT/UPDATE in `SqlEngine`,
  `DomainSqlEngine`, `HistorySqlEngine`, `MvccSqlEngine`,
  `DurableMvccStore`. Each engine needs its own integration
  slice that scans its row set under the write lock.
- **Disk-backed secondary index** — the enforcer scans
  existing rows O(N). A B+tree secondary index requires the
  ADR 0096 mutable-storage foundation + Gate 3 work.
- **Deferred constraints** — all checks are immediate.
- **FOREIGN KEY enforcement** — a separate ADR.

## 3. Compliance

1. Empty unique_keys → every check passes.
2. Single-column duplicate → `UniqueViolation {constraint, columns}`.
3. Distinct values → passes.
4. NULL in a UNIQUE column → candidate skips that constraint.
5. Composite key rejects full-tuple duplicate.
6. Multiple keys → first-wins (deterministic from input order).
7. 6 unit tests in `src/unique_check.rs::tests`.
