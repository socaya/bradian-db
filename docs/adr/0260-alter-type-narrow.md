# ADR 0260 — ALTER COLUMN TYPE narrowing via implicit CAST

## Status

Accepted.

## Context

Widening (ADR 0130) was the only type-change path. Narrowing
(BIGINT → INT, TEXT → BIGINT, …) was not reachable from SQL,
so schemas were effectively append-only on type precision.
P0-1 asks for the symmetric narrowing path with explicit row
validation.

## Decision

- Unified `ALTER TABLE t ALTER COLUMN c TYPE <target>` flow:
  1. Scan all rows.
  2. Cast each row's column value via the existing
     `cast_value`. Any single row that fails to cast aborts
     the ALTER with `SqlError::Type` — nothing is mutated.
  3. Flip the catalog type. If the target is a declared
     widening (ADR 0130), use `widen_column_type` (fast path).
     Otherwise use the new `change_column_type` which permits
     any target but drops any literal default on the column.
  4. Rewrite the TableStore META_KEY schema pin so the store
     accepts the new type for subsequent writes.
  5. Replace each row through the new-schema validator.
- New `Catalog::change_column_type` (also surfaces on
  `PersistentCatalog` via `Command::ChangeColumnType`).
  Rejects on dependency edges (FK, UK, PK, incoming FK).
- Widen path also gains the TableStore pin rewrite so a
  widened column can be read back without reopen reshape.

## Consequences

- Narrowing is opt-in per ALTER — users explicitly accept
  that each row must fit. No auto-truncation.
- Dependency edges still block (same rule as widen). The
  dependency planner must be run first for FK/UK columns.
- Literal defaults on the column are dropped during a type
  change — the stored literal may no longer be valid. The
  caller must re-set an appropriate default after.
- Expression defaults (ADR 0259) are preserved — they are
  evaluated at INSERT time against the new type.
- 8 cases in `tests/alter_type_narrow.rs` cover:
  BIGINT→INT when values fit; BIGINT→INT overflow rejected;
  TEXT→BIGINT numeric; TEXT→BIGINT non-numeric rejected;
  BIGINT→TEXT; persists across reopen; widen still works
  (now with pin rewrite); PK / dependents reject.
