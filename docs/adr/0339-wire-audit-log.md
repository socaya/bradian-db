# ADR 0339 — Audit-log integration at the wire handler

## Status

Accepted. Puts `bradian_security::audit::AuditLog` (which
has existed since the security crate bootstrap) to real
use at the wire layer so every statement, denial, and
session boundary lands in an operator-readable trail.

## Context

ADR 0334 (authz) and ADR 0337 (memory pressure) both
refuse statements with typed errors but leave no record.
An operator responding to an incident has no way to
answer "which principal tried to run what, and why was
it refused?" — the kind of question auditors always ask.

`bradian_security::AuditLog` already ships a JSONL
append-only writer with a typed `AuditEntry`; the
missing piece is the integration at the one place the
authz + pressure decisions are made.

## Decision

`WireServer` gains an optional `Arc<AuditLog>`:

```rust
pub fn with_audit_log(mut self, log: Arc<AuditLog>) -> Self;
```

When set, the handler writes an `AuditEntry` at each of
the following points:

| Event                    | action            | target        | detail                          |
|--------------------------|-------------------|---------------|---------------------------------|
| HelloOk sent             | `session_open`    | `session=N`   | —                               |
| authz predicate = false  | `authz_deny`      | `sql:VERB`    | truncated SQL (≤256 bytes)      |
| memory pressure refused  | `pressure_deny`   | `sql:VERB`    | truncated SQL                   |
| engine `Rows` returned   | `execute_rows`    | `sql:VERB`    | `rows=N; sql=…`                 |
| engine `Command` returned| `execute_command` | `sql:VERB`    | `affected=N; sql=…`             |
| engine error             | `execute_error`   | `sql:VERB`    | `err=…; sql=…`                  |
| Bye received             | `session_close`   | `session=N`   | `bye`                           |
| Peer disconnected / IO   | `session_close`   | `session=N`   | `eof` or error text             |

`principal_id` is the mTLS-derived principal from ADR
0333 when `serve_one_mtls*` is used; plaintext + TLS
paths record as `anonymous`.

Audit-write failures are logged (`tracing::warn!`) but
**not propagated** — a temporarily unwritable audit log
must not terminate the connection; the alternative
trades one compliance signal for a live-service outage.

## Scope

**In scope:**
- Audit emission at every decision point above.
- `principal_id = "anonymous"` on non-mTLS paths.
- Target string `sql:VERB` derived by the statement's
  first whitespace-delimited token (upper-cased); long
  literals are truncated to 256 bytes with a trailing `…`.
- 6 new tests (30 total in wire):
  1. Plaintext session records session_open, execute_rows,
     session_close with principal "anonymous".
  2. mTLS Analyst INSERT records an `authz_deny` entry
     with the mapped principal id "bob" and target
     "sql:INSERT".
  3. Memory-pressure refusal records a `pressure_deny`
     entry with target "sql:SELECT".
  4. No audit log installed → no entries, no panic
     (regression guard).
  5. `truncate_sql` unit test.
  6. `audit_target_of` unit test (case + whitespace).

**Out of scope** (dedicated follow-ups):
- `plan_fingerprint` population — requires the plan
  layer (ADR 0296) to compute a BLAKE3 of the compiled
  plan and pass it up through `SqlResultSet`. A one-
  slice follow-up.
- Structured SQL target (table name list) instead of
  just the verb. That needs real parser integration,
  not the prefix hack this slice uses.
- Audit-log rotation / retention — the `AuditLog`
  current API is append-only on one file. Rotation
  is an operator concern; a built-in rotator is a
  separate slice.

## Consequences

- An operator wiring `.with_audit_log(a)` into their
  `WireServer` gets a complete JSONL trail of every
  authenticated or anonymous session, including every
  refusal class with its principal, verb, and
  (truncated) SQL text — the shape auditors expect.
- 30 wire tests total (6 new). Workspace stays green
  under `cargo clippy --workspace --all-targets
  --locked -- -D warnings`.
