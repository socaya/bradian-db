# ADR 0345 — Statement fingerprint in audit entries

## Status

Accepted. Populates the `AuditEntry::plan_fingerprint`
field that has been shipped (but always `None`) since
the audit log bootstrap.

## Context

ADR 0339 wired the audit log at the wire handler but
hardcoded `plan_fingerprint: None` because the wire
layer doesn't run the planner. The field is where an
operator correlates a statement across runs and across
versions — if two sessions report different fingerprints
for what looks like the same SQL, something upstream
drifted.

## Decision

New helper `sql_fingerprint(sql) -> String` normalises
the SQL (trim, collapse runs of whitespace to a single
ASCII space, lowercase) and BLAKE3-hashes the result,
returning 64 hex chars.

Each `audit_record(...)` call at an Execute decision
point passes `Some(sql_fingerprint(&sql))`:

| Site            | Fingerprint       |
|-----------------|-------------------|
| `authz_deny`    | Some              |
| `pressure_deny` | Some              |
| `execute_rows`  | Some              |
| `execute_command`| Some             |
| `execute_error` | Some              |
| `session_open`  | None (no SQL)     |
| `session_close` | None (no SQL)     |

The `audit_record` function signature grows one
parameter: `plan_fingerprint: Option<String>`.

## Scope

**In scope:**
- `sql_fingerprint` via BLAKE3 over a normalised SQL
  string. Whitespace + case insensitive so a client that
  pretty-prints its queries differently across runs
  still gets the same fingerprint for equivalent
  statements.
- Thread the fingerprint through every Execute-path
  audit entry.
- 3 new tests (39 total in wire):
  1. End-to-end SELECT → `execute_rows` entry carries a
     64-char hex `plan_fingerprint`; session_* entries
     have it null.
  2. `sql_fingerprint` is whitespace + case insensitive
     (same hash across formatting variants); different
     statements hash to different values.
  3. Output length is 64 chars of hex.

**Out of scope** (dedicated follow-ups):
- **Compiled-plan** fingerprinting via
  `bradian_planner::sql_plan` (ADR 0296). The engine
  doesn't yet run that planner in-line with execute, so
  there is no plan tree to hash at this call site. The
  SQL-text fingerprint is deliberately labelled as such
  in the ADR body so operators don't confuse it with
  "the optimizer chose plan X" telemetry.
- Parameter-stripping normalisation (so `WHERE id = 1`
  and `WHERE id = 2` fingerprint the same — a Postgres
  pg_stat_statements-style normalisation). That belongs
  with the planner integration.

## Consequences

- An operator grepping the audit JSONL for a given
  `plan_fingerprint` sees every session that ran an
  equivalent statement.
- No new workspace deps — `blake3` was already pulled in
  across the workspace.
- 39 wire tests green; workspace stays green under
  `cargo clippy --workspace --all-targets --locked
  -- -D warnings`.
