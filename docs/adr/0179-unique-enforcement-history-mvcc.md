# ADR 0179 — UNIQUE enforcement in `HistorySqlEngine` + `MvccSqlEngine` (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-21.** Final SQL-engine strand
of BRD-REL-002 UNIQUE enforcement after ADRs 0176 (SqlEngine
INSERT), 0177 (SqlEngine UPDATE) and 0178 (DomainSqlEngine).

## 1. Scope

Mirrors the scan-and-check pattern into:

- `HistorySqlEngine::execute_inner` INSERT + UPDATE
  branches. `tx.scan(table.id)` reads the current
  (post-retention-floor) rows under the active
  historical transaction; historical AS OF reads
  are unaffected. UPDATE uses the expression
  evaluator from `sql_expression`; the UNIQUE check
  runs **after** expression evaluation on the final
  mutated row.
- `MvccSqlEngine::execute_inner` INSERT + UPDATE
  branches. Identical pattern against
  `TypedMvccStore` transactions.

UPDATE in both engines excludes the row being updated
from the scan by PK projection, matching ADR 0177.
`SqlError::Unique(UniqueViolation)` (ADR 0176) surfaces
violations identically across all four engines.

## 2. Out of scope

- **`DurableMvccStore`.** Has no SQL surface; raw
  key/value API callers are responsible for their own
  uniqueness discipline.
- **Disk-backed secondary index.** Still scan-based;
  ADR 0127 non-unique helper stays unwired.
- **Historical AS OF uniqueness.** AS OF SELECT reads
  are unchanged; UNIQUE is a mutation-time check, not
  a snapshot-time one.
- **Concurrent cross-session guarantees.** The
  commit-mutex / per-key MVCC conflict path remains
  the authoritative cross-session serialization; this
  slice adds per-statement UNIQUE semantics within
  one transaction's view.

## 3. Compliance

Two new test files:

`crates/bradian-relational/tests/unique_history_sql.rs`
(5 tests, all green):

1. `history_duplicate_insert_is_rejected`.
2. `history_update_collision_is_rejected`.
3. `history_self_update_is_allowed`.
4. `history_insert_distinct_unique_value_succeeds`.
5. `history_update_to_brand_new_unique_value_succeeds`.

`crates/bradian-relational/tests/unique_mvcc_sql.rs`
(4 tests, all green):

1. `mvcc_duplicate_insert_is_rejected`.
2. `mvcc_update_collision_is_rejected`.
3. `mvcc_self_update_is_allowed`.
4. `mvcc_insert_distinct_unique_value_succeeds`.

Clippy clean on `-D warnings`. All earlier
UNIQUE / DML tests continue to pass.

## 4. Roadmap impact

BRD-REL-002 UNIQUE enforcement across every Bradian
SQL surface is now complete at Gate 1 scope:

| Engine | INSERT | UPDATE |
|---|---|---|
| `SqlEngine` | ADR 0176 | ADR 0177 |
| `DomainSqlEngine` | ADR 0178 | ADR 0178 |
| `HistorySqlEngine` | **ADR 0179** | **ADR 0179** |
| `MvccSqlEngine` | **ADR 0179** | **ADR 0179** |

Remaining BRD-REL-002 work (CHECK, FOREIGN KEY) stays
explicitly open. BRD-STO-001 disk-backed secondary
indexes remain Gate 3+ (mutable-storage prerequisite).
