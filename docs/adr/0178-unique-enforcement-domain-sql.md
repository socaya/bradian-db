# ADR 0178 — UNIQUE enforcement in `DomainSqlEngine` (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-21.** Follow-up to ADRs 0176 /
0177, which closed UNIQUE enforcement on `SqlEngine` INSERT
and UPDATE. ADR 0176 §2 listed `DomainSqlEngine` wiring as
deferred.

## 1. Scope

Mirrors the ADR 0176 + 0177 pattern into
`DomainSqlEngine::execute_inner`:

- **INSERT path** (`"insert"` branch in the `insert | select
  | update | delete` block): when the resolved table carries
  `unique_keys`, scan the table under the active transaction
  via `tx.scan(table.id)?` and run
  `UniqueConstraintChecker::check(candidate, existing)`
  before `tx.insert`.
- **UPDATE path** (`"update"` branch): when the resolved
  table carries `unique_keys`, scan the table, filter out
  the row being updated by PK projection, and run the
  checker on the mutated row before `tx.replace`.
- `SqlError::Unique(UniqueViolation)` (introduced in ADR
  0176) surfaces violations identically across engines.
- Multi-table isolation: the checker loads only the single
  table's rows via `tx.scan(table.id)`; constraints on
  table A never see table B's rows, even though they share
  the WAL / domain.

## 2. Out of scope

- **`HistorySqlEngine` / `MvccSqlEngine` UNIQUE wiring.**
  Deferred; each has its own session discipline.
- **Disk-backed secondary index.** Enforcement remains
  scan-based; ADR 0127 helper stays unwired.
- **Transaction-level uniqueness guarantees across
  concurrent writers.** `DomainSqlEngine` serializes DML
  under the session; cross-session conflict detection is
  the shared-WAL commit mutex's job (unchanged).

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/unique_domain_sql.rs`
(5 tests, all green):

1. `duplicate_insert_in_shared_domain_is_rejected` — two
   INSERTs with the same unique value in the same
   transaction → `SqlError::Unique(_)`.
2. `update_collision_in_shared_domain_is_rejected` — row
   1's email updated to row 2's email after both are
   committed → `SqlError::Unique(_)`.
3. `self_update_in_shared_domain_is_allowed` — row 1's
   email updated to its own value → ok (PK filter
   excludes self).
4. `unique_on_one_table_does_not_affect_sibling_table_inserts`
   — table `v` has no UNIQUE; two rows with equal
   non-unique `label` coexist; table `u` enforces
   UNIQUE(email) alongside.
5. `rollback_leaves_no_visible_row_for_subsequent_unique_check`
   — INSERT + ROLLBACK, then fresh INSERT of the same
   value → succeeds (rolled-back row is invisible to
   the scan).

Clippy clean on `-D warnings`. All earlier tests
continue to pass.

## 4. Roadmap impact

BRD-REL-002 UNIQUE enforcement coverage after this
ADR:

| Engine | INSERT | UPDATE |
|---|---|---|
| `SqlEngine` | ADR 0176 | ADR 0177 |
| `DomainSqlEngine` | **ADR 0178** | **ADR 0178** |
| `HistorySqlEngine` | open | open |
| `MvccSqlEngine` | open | open |

`DurableMvccStore` has no SQL surface and does not
participate.
