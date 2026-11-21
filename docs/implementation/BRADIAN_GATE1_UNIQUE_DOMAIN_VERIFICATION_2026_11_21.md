# Master Gate 1 — UNIQUE in `DomainSqlEngine` verification

21 November 2026. ADR 0178. Follow-up to ADRs 0176 / 0177
that extended UNIQUE enforcement from `SqlEngine` into the
shared-WAL multi-table session engine.

## Executed result

| Executed check | Result |
|---|---|
| `cargo test --locked -p bradian-relational --test unique_domain_sql` | 5 passed, 0 failed |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## Finding and action

ADR 0176/0177 wired UNIQUE at INSERT + UPDATE on
`SqlEngine` only; `DomainSqlEngine` still bypassed the
check. The same scan-and-check pattern translates
directly because the active transaction exposes
`tx.scan(table.id)` for per-table reads.

Change (ADR 0178):

- `DomainSqlEngine::execute_inner` INSERT branch: when
  `table.unique_keys` is non-empty, run
  `UniqueConstraintChecker::check(candidate,
  tx.scan(table.id)?)` before `tx.insert`.
- UPDATE branch: filter the row being updated by PK
  projection from `tx.scan(table.id)?` and run the
  checker on the mutated row before `tx.replace`.
- `SqlError::Unique(UniqueViolation)` (ADR 0176)
  surfaces violations identically across engines.
- Multi-table isolation preserved: the checker reads
  only the table under constraint; sibling tables in
  the same WAL/domain are unaffected.

Added tests
(`crates/bradian-relational/tests/unique_domain_sql.rs`,
5 total):

1. `duplicate_insert_in_shared_domain_is_rejected`.
2. `update_collision_in_shared_domain_is_rejected`.
3. `self_update_in_shared_domain_is_allowed` (PK filter
   excludes self).
4. `unique_on_one_table_does_not_affect_sibling_table_inserts`.
5. `rollback_leaves_no_visible_row_for_subsequent_unique_check`.

## Scope limits (honest)

- `HistorySqlEngine` and `MvccSqlEngine` UNIQUE wiring
  remain open. `DurableMvccStore` has no SQL surface.
- Enforcement is still scan-based (O(N) per INSERT/
  UPDATE); ADR 0127 non-unique secondary index helper
  stays unwired.
- Cross-session UNIQUE serialization is the shared-WAL
  commit mutex's job (unchanged); no additional
  concurrent-writer guarantee introduced here.

## Evidence

- Source:
  `crates/bradian-relational/src/domain_sql.rs` —
  INSERT and UPDATE branches extended with
  `UniqueConstraintChecker` calls.
- Tests:
  `crates/bradian-relational/tests/unique_domain_sql.rs`.
- ADR: `docs/adr/0178-unique-enforcement-domain-sql.md`.
