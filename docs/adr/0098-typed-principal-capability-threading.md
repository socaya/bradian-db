# ADR 0098 — Typed authorization capabilities foundation (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-15 as a Gate-1-scope foundation
for INV-003.**

## 1. Honest scope

INV-003 (BRD-SEC-008) — "authorization context propagation
across every access path" — is a release-blocking MUST that
requires **enterprise identity + TLS/mTLS + per-user key
custody**, all of which still need §13 owner decisions. This
ADR does NOT satisfy INV-003. It ships the **typed primitive**
INV-003's enforcement will be built on:

- `bradian_core::authz::Capability` enum: `Read`, `Write`,
  `Ddl`, `Admin`.
- `bradian_core::authz::AuthorizationContext { principal_id,
  capabilities }`.
- `Capability::required_for_sql(sql)` — classifies the first
  keyword of a SQL statement.
- `AuthorizationContext::check_sql(sql)` — fast-fail check
  returning `AuthorizationDenied` when the context lacks the
  required capability.
- `SqlEngine::execute_as(authz, sql)` — authz-aware execute
  that pre-checks before dispatching to `execute()`.
- Factory helpers: `administrator()` and `read_only()`.

What remains open (explicit non-goals per CLAUDE.md §2 and §6):
- **Enterprise identity + directory integration** (§13).
- **TLS/mTLS wire authentication** (§13).
- **Per-user key custody + rotation** (§13).
- **Row-level / column-level ACLs** (BRD-SEC-010).
- **Authorization propagation through distributed SQL
  fragments** (Gate 5).
- **Audit trail of authz decisions** (BRD-SEC-005; separate
  ADR, future).
- **The temporary `BRADIAN_API_TOKEN` is still loopback-only
  per CLAUDE.md §6**; `execute_as` is a code-level check,
  not a wire-protocol authentication.

## 2. Compliance

1. `Capability::required_for_sql("SELECT 1")` → `Some(Read)`.
2. `Capability::required_for_sql("INSERT …")` → `Some(Write)`.
3. `Capability::required_for_sql("CREATE …")` → `Some(Ddl)`.
4. `Capability::required_for_sql("")` → `None` (parser
   produces real error).
5. `administrator()` allows everything.
6. `read_only()` rejects writes and DDL with a clear
   `AuthorizationDenied` error containing the required
   capability.
7. `SqlEngine::execute_as(read_only, "CREATE DATABASE x")`
   → `SqlError::Authz(AuthorizationDenied { … })`.
8. Unknown keywords pass through so the SQL parser produces
   the real error.
9. Clippy clean.
10. Backward compatible: existing `execute(sql)` unchanged.
