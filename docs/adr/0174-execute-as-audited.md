# ADR 0174 — `SqlEngine::execute_as_audited` wires AuditChain into the authz-gated path (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-21.** Closes the
code-writable portion of BRD-SEC-005 audit integration
that ADR 0104 §2 called out:

> **Automatic integration with `SqlEngine::execute_as`** —
> a future slice wires the log into the authz-gated path.

ADR 0098 shipped `execute_as` for authz fast-fail; ADR
0118 shipped the BLAKE3 hash-chained `AuditChain`; the
glue was never written. This ADR adds it.

## 1. Scope

New method on `SqlEngine`:

```rust
pub fn execute_as_audited(
    &self,
    authz: &bradian_core::AuthorizationContext,
    sql: &str,
    chain: &crate::audit_chain::AuditChain,
) -> SqlResult<SqlResultSet>
```

Behavior on every call:

1. Capture wall-clock `timestamp_ms` (ms since epoch; `0`
   if the system clock is pre-epoch).
2. Call `authz.check_sql(sql)`:
   - On `Err(denied)`: outcome =
     `ChainedOutcome::AuthzDenied(denied.to_string())`;
     wrap the error as `SqlError::Authz(denied)` for the
     eventual return.
   - On `Ok`: call `self.execute(sql)`:
     - On `Ok(rs)`: outcome = `ChainedOutcome::Ok`.
     - On `Err(e)`: outcome =
       `ChainedOutcome::Err(e.to_string())`; preserve
       the original error for return.
3. Append one `ChainedAuditEntry` carrying
   `{ timestamp_ms, principal_id, sql, outcome }` to the
   chain. If the append fails, discard the SQL result and
   return `SqlError::Audit(msg)` instead — never silently
   drop an audit gap.
4. Otherwise return the preserved SQL result (or
   SqlError).

New `SqlError::Audit(String)` variant for the append-
failure branch.

**ADR 0098 fast-fail-on-denial is preserved.** The
only behavioral change from `execute_as` is that each
call ends with a chain append before the error is
re-raised, so an operator can prove which denied call
each entry corresponds to.

## 2. Out of scope

- **Enforcement at call sites.** This ADR adds the
  capability; callers choose when to use it. ADR 0098
  `execute_as` continues to work unaudited.
- **Rotation / retention.** The chain is append-only and
  grows unbounded. ADR 0104 §2 "Access-controlled
  retention" remains OPEN — operator FS permissions +
  periodic snapshot-and-reset remain the discipline.
- **WORM storage.** Local file only.
- **Externally anchored integrity.** An attacker with
  full file write can rebuild the chain end-to-end
  (ADR 0118 §2). Anchoring to an external log remains
  OPEN.
- **Automatic wiring into DomainSqlEngine /
  HistorySqlEngine / MvccSqlEngine.** Only `SqlEngine`
  for now; the three session engines accept a session-
  level audit callback in a follow-up ADR if an operator
  needs it.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/sql_engine_audited.rs`
(7 tests, all green):

1. `successful_statement_appends_ok_record` — Ok outcome.
2. `authz_denial_records_authz_denied_outcome_and_surfaces_sql_error_authz`
   — Reader principal writing INSERT: chain records
   `AuthzDenied(...)`; caller still sees `SqlError::Authz(_)`.
3. `downstream_statement_error_records_err_outcome` —
   syntax error passes authz but fails `execute`; chain
   records `Err(msg)`.
4. `multiple_statements_produce_contiguous_hash_chain`
   — four statements; `chain.verify()` walks all four
   without `ChainError::Tamper`.
5. `audit_chain_is_persistent_across_reopen` — the chain
   survives Drop and reopen.
6. `principal_id_is_recorded_verbatim` — three distinct
   principals in a row; each record carries the exact
   `principal_id()` string.
7. `tamper_evident_chain_detects_altered_body` —
   byte-flip mid-record; `verify()` returns a `Tamper`,
   `Corruption` or `Serde` variant depending on where
   the flip lands. Three honest failure paths; the
   invariant is that `verify()` never returns Ok on a
   mutated chain.

Full `bradian-relational` suite remains green;
clippy clean on `-D warnings`.

## 4. Roadmap impact

BRD-SEC-005 "native audit log" closes the code-writable
"wire into authz-gated path" obligation. Remaining
BRD-SEC-005 work is operational: externally anchored
integrity, WORM storage, retention lifecycle. ADR 0104
§2's enumerated open items that remain:

> **Externally anchored integrity**, **Immutable
> storage**, **Access-controlled retention**.

None of those are code-writable inside this crate
without an owner decision on anchor and retention policy
(see `BRADIAN_COMPREHENSIVE_GAPS_ANALYSIS.md` §7 for the
owner-blocker list).
