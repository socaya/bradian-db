# ADR 0280 + 0282 — Multi-table FROM authz + revocation propagation

## Status

Accepted.

## Context

ADR 0258 and 0278 covered single-table per-object grants.
Two gaps remained: (1) joined SELECTs against multiple
tables elevated only on the FROM's first target, not every
referenced table; (2) a REVOKE issued mid-session did not
take effect until the next session started.

## Decision

- Rename `classify_single_table_statement` wrapper →
  `classify_statement_targets(sql) -> (cap, obj_cap,
  Vec<Vec<String>>)`. For SELECT, calls new
  `extract_select_table_targets(sql)` which walks the FROM
  clause and every JOIN target. For INSERT / UPDATE /
  DELETE, returns the single target as a 1-element vec.
- `AuthenticatedSession::execute` now re-reads the live
  user catalog's grants on every call (ADR 0282). The
  session's at-start snapshot moves to a `_initial_grants`
  field kept for diagnostics.
- Elevation requires EVERY referenced table to carry the
  per-object grant. A single missing grant blocks the
  elevation; the statement then runs with the user's
  global capabilities (and typically fails authz
  fast-fail).

## Consequences

- Multi-table SELECTs (joins) now behave correctly: no
  elevation unless the user holds SELECT on every
  referenced table. Partial grants fail-safe.
- REVOKE takes effect on the next statement in the SAME
  session. Expected throughput hit per statement is one
  extra `users.snapshot()` call which is cheap (in-memory
  BTreeMap clone).
- Scope limits:
  - FROM extraction is heuristic text-walking — it doesn't
    recurse into subqueries. A subquery inside a FROM
    (`FROM (SELECT ...) AS t`) is treated as one table
    reference; the IN-subquery rewrite path (ADR 0229)
    runs before extraction so the inner is already a
    literal list.
  - DDL statements (CREATE TABLE etc.) are not covered —
    those need ADR 0279 per-object DDL grants.
- 3 cases in `tests/multi_table_authz_and_revoke.rs`:
  join requires grants on every table; join allowed when
  both; REVOKE propagates mid-session.
