# ADR 0279 — Per-object ALTER / DROP grants on tables

## Status

Accepted.

## Context

ADRs 0258 and 0278 covered per-object SELECT and
INSERT/UPDATE/DELETE. DDL (ALTER / DROP) stayed global-only
until this ADR.

## Decision

- `ObjectCapability` gains `Alter` and `Drop` variants.
- `UserRecord::allows_on_table` extends the "global
  capability implies per-object" rule: `Capability::Ddl`
  implies `Alter` + `Drop` on every table.
- `detect_object_capability` adds `alter` / `drop` keyword
  recognition. SQL grammar now accepts
  `GRANT { SELECT | INSERT | UPDATE | DELETE | ALTER | DROP }
  ON <qualified-table> TO <user>` (same for REVOKE).
- `classify_single_table_statement` classifies
  `ALTER TABLE <name> …` and `DROP TABLE <name>` to the new
  capabilities. Session elevation elevates `Capability::Ddl`
  for a single call when the per-table grant is present.

## Consequences

- Covers ALTER / DROP on tables. CREATE grants (per schema /
  per database) are a separate slice — different scope since
  CREATE doesn't have a pre-existing object to grant against.
- 6 cases in `tests/per_object_ddl_grants.rs` cover:
  no-grant blocks ALTER; GRANT ALTER enables RENAME + SET
  NOT NULL; ALTER grant does NOT enable DROP; admin global
  DDL bypasses; REVOKE propagates to next statement; GRANT
  DROP persists across reopen.
