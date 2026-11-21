# Master Gate 1 — `SqlEngine::execute_as_audited` verification

21 November 2026. ADR 0174. BRD-SEC-005 audit-integration
closure; **non-breaking** (adds a new method and a new
`SqlError::Audit` variant; existing `execute_as` unchanged).

## Executed result

| Executed check | Result |
|---|---|
| `cargo test --locked -p bradian-relational --test sql_engine_audited` | 7 passed, 0 failed |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## Finding and action

ADR 0104 shipped `AuditLog` (CRC32 append-only) and ADR
0118 shipped `AuditChain` (BLAKE3 hash-chained tamper
evidence). ADR 0104 §2 explicitly noted "Automatic
integration with `SqlEngine::execute_as` — a future slice
wires the log into the authz-gated path." That follow-up
never happened. Today `SqlEngine::execute_as` invokes
`authz.check_sql(sql)?` and delegates to `execute(sql)`
without appending anything.

Change (ADR 0174):

- New `SqlEngine::execute_as_audited(authz, sql, chain)`
  method. Every call appends exactly one
  `ChainedAuditEntry` carrying
  `{ timestamp_ms, principal_id, sql, outcome }`.
- Outcome maps:
  - Authz denial → `ChainedOutcome::AuthzDenied(msg)`.
  - Downstream statement error →
    `ChainedOutcome::Err(msg)`.
  - Success → `ChainedOutcome::Ok`.
- ADR 0098 fast-fail-on-denial preserved: the caller
  still sees `SqlError::Authz(_)` after the audit record
  lands.
- New `SqlError::Audit(String)` variant: if the audit
  append itself fails, the SQL result is discarded and
  the audit failure surfaces instead — no silent chain
  gap.

Added tests (7 in
`crates/bradian-relational/tests/sql_engine_audited.rs`):

1. `successful_statement_appends_ok_record`.
2. `authz_denial_records_authz_denied_outcome_and_surfaces_sql_error_authz`
   — reader attempting INSERT: chain records
   `AuthzDenied(...)`, caller still sees
   `SqlError::Authz(_)`.
3. `downstream_statement_error_records_err_outcome` —
   syntax error passes authz but fails `execute`.
4. `multiple_statements_produce_contiguous_hash_chain`
   — four statements; `chain.verify()` returns 4 records
   with no `ChainError::Tamper`.
5. `audit_chain_is_persistent_across_reopen` — chain
   survives Drop and reopen.
6. `principal_id_is_recorded_verbatim` — three distinct
   principals; each record carries the exact string.
7. `tamper_evident_chain_detects_altered_body` —
   byte-flip inside a record; verify fails with
   `Tamper`, `Corruption` or `Serde` (depending on which
   byte flips). Invariant: `verify()` never returns Ok
   on a mutated chain.

## Scope limits (honest)

- **Operator decides when to call `execute_as_audited`**.
  `execute_as` continues to work unaudited; this slice
  does not force the audited variant.
- **Externally anchored integrity**: not implemented. An
  attacker with full file write can rebuild the chain
  end-to-end; cf. ADR 0118 §2.
- **WORM storage**: plain file on local disk.
- **Access-controlled retention**: operator must enforce
  FS permissions and rotation; no automatic retention
  lifecycle.
- **`DomainSqlEngine` / `HistorySqlEngine` /
  `MvccSqlEngine` audited variants**: deferred. Only
  `SqlEngine` in this slice.

## Evidence

- Source:
  `crates/bradian-relational/src/sql.rs` — new
  `execute_as_audited` method (public API), new
  `SqlError::Audit` variant.
- Tests:
  `crates/bradian-relational/tests/sql_engine_audited.rs`
  — 7 round-trip + tamper + persistence tests.
- ADR: `docs/adr/0174-execute-as-audited.md`.
