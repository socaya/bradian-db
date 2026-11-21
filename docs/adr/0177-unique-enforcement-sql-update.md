# ADR 0177 — UNIQUE enforcement on `SqlEngine` UPDATE (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-21.** Follow-up to ADR 0176.
Closes the UPDATE strand of BRD-REL-002 UNIQUE enforcement
on `SqlEngine`. ADR 0176 §2 noted:

> **UPDATE path.** UPDATE's unique-enforcement is a separate
> slice that must exclude the row being updated from the
> existing-rows iterator. Deferred.

This ADR delivers that slice.

## 1. Scope

- `SqlEngine::execute_impl` UPDATE path: after applying the
  SET assignments to the fetched row and before
  `TableStore::replace`, scan all rows and run
  `UniqueConstraintChecker::check` with an iterator that
  **filters out the row being updated** (matched by its
  primary-key projection).
- Primary key cannot be modified by UPDATE (already
  enforced earlier in the branch), so the PK projection
  used for the filter is stable across the pre- and
  post-mutation states.
- No API change on `UniqueConstraintChecker`: the filter
  is applied inline at the call site. Keeping the checker
  signature narrow avoids leaking storage-path concerns
  into the primitive.

## 2. Out of scope

- **`DomainSqlEngine` / `HistorySqlEngine` /
  `MvccSqlEngine` UPDATE enforcement.** Each will need
  its own integration slice; the row-set scan and the
  PK filter translate directly, but the engines have
  distinct write-lock disciplines.
- **Multi-row UPDATE.** The current UPDATE path only
  matches a single PK, so the self-filter is trivially
  one-to-one. A future non-PK WHERE expansion will
  need to filter by the set of matched PKs.
- **Disk-backed secondary index.** Enforcement is still
  scan-based; ADR 0127 `NonUniqueIndex` remains unwired.
- **Deferred / deferrable constraints.** Immediate check
  only.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/unique_update_sql.rs`
(8 tests, all green):

1. `update_non_unique_column_is_always_allowed`.
2. `update_unique_column_to_brand_new_value_is_allowed` —
   confirmed via round-trip `SELECT`.
3. `update_unique_column_to_another_rows_current_value_is_rejected`
   — returns `SqlError::Unique { constraint: "uq_email",
     … }`.
4. `update_unique_column_to_its_own_current_value_is_allowed`
   — the self-row is filtered by PK from the existing
   iterator; the "collision with self" is a non-event.
5. `update_composite_unique_partial_overlap_is_allowed`
   — changing one component of a composite tuple so the
   new tuple is distinct from any peer.
6. `update_composite_unique_full_tuple_collision_is_rejected`.
7. `update_where_matches_nothing_is_a_noop_and_never_enforces`
   — WHERE missing PK → affected=0; enforcement is
   skipped (no mutation happens).
8. `update_to_null_in_unique_nullable_column_does_not_collide`
   — SQL-standard NULL semantics apply equally to
   UPDATE-to-NULL.

Clippy clean on `-D warnings`. All existing tests
continue to compile.

## 4. Roadmap impact

BRD-REL-002 UNIQUE enforcement on `SqlEngine`: INSERT
closed (ADR 0176) + UPDATE closed (ADR 0177).
`DomainSqlEngine` / `HistorySqlEngine` /
`MvccSqlEngine` UPDATE and INSERT enforcement remain
explicitly open. BRD-STO-001 secondary indexes are
unaffected.
