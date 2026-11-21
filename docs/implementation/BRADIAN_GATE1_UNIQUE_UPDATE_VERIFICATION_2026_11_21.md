# Master Gate 1 — `UNIQUE` at SqlEngine UPDATE verification

21 November 2026. ADR 0177. UPDATE-path follow-up to
ADR 0176; closes the BRD-REL-002 UNIQUE enforcement scope
on `SqlEngine`.

## Executed result

| Executed check | Result |
|---|---|
| `cargo test --locked -p bradian-relational --test unique_update_sql` | 8 passed, 0 failed |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## Finding and action

ADR 0176 wired `UniqueConstraintChecker` into `SqlEngine`
INSERT but explicitly deferred UPDATE: "UPDATE's unique-
enforcement is a separate slice that must exclude the row
being updated from the existing-rows iterator." Without it,
a workflow `INSERT then UPDATE row's unique column`
bypassed the constraint.

Change (ADR 0177):

- `SqlEngine::execute_impl` UPDATE path: after applying
  SET assignments and before `TableStore::replace`, scan
  all rows and run `UniqueConstraintChecker::check` with
  the row being updated filtered out by PK projection.
- Primary key cannot change (already enforced earlier in
  the branch), so the PK filter is stable across the
  pre- and post-mutation states.
- No API change on `UniqueConstraintChecker` — the
  filter is applied inline at the call site.
- `SqlError::Unique` (introduced in ADR 0176) surfaces
  violations identically to the INSERT path.

Added tests
(`crates/bradian-relational/tests/unique_update_sql.rs`,
8 total):

1. `update_non_unique_column_is_always_allowed`.
2. `update_unique_column_to_brand_new_value_is_allowed`.
3. `update_unique_column_to_another_rows_current_value_is_rejected`.
4. `update_unique_column_to_its_own_current_value_is_allowed`
   (self-filter proves the PK comparison).
5. `update_composite_unique_partial_overlap_is_allowed`.
6. `update_composite_unique_full_tuple_collision_is_rejected`.
7. `update_where_matches_nothing_is_a_noop_and_never_enforces`.
8. `update_to_null_in_unique_nullable_column_does_not_collide`
   — SQL-standard NULL semantics on UPDATE-to-NULL.

## Scope limits (honest)

- **`SqlEngine` only.** `DomainSqlEngine` /
  `HistorySqlEngine` / `MvccSqlEngine` UPDATE and
  INSERT enforcement remain explicitly open.
- **Single-PK UPDATE only.** Current grammar matches
  exactly one row by PK; the self-filter is one-to-one.
  A future non-PK WHERE expansion would need to filter
  by the set of matched PKs.
- **O(N) scan per UPDATE** (plus the existing O(N) scan
  per INSERT).
- **No deferred / deferrable constraints.** Immediate
  check only.
- **No column-level TYPE UNIQUE.** Table-level
  `UNIQUE (col[, ...])` only (per ADR 0176).

## Evidence

- Source:
  `crates/bradian-relational/src/sql.rs` — UPDATE
  branch extended with the self-filtered UNIQUE check.
- Tests:
  `crates/bradian-relational/tests/unique_update_sql.rs`
  (8 accept/reject/null/composite/no-op tests).
- ADR: `docs/adr/0177-unique-enforcement-sql-update.md`.
