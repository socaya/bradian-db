# ADR 0123 — Row/Column/Tenant access-policy primitives (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch I I3-I5 — BRD-SEC-002
foundation.

## Scope

- `RowLevelPolicy { predicate: RowPredicate }` with
  `ColumnEquals / ColumnNotEquals / AlwaysAllow /
  AlwaysDeny`.
- `ColumnLevelPolicy { readable: BTreeSet<String> }` with
  `allow_all / deny_all / project` for projection-side
  filtering.
- `TenantIsolation { tenant_column, allowed_tenants }` with
  `single_tenant` factory + null/non-string rejection.
- `TableAccessPolicy` composing all three, evaluated with an
  `AuthorizationContext` for future capability-aware hooks.

## Out of scope

- **Policy storage in the catalog** — where do policies
  live on disk? A follow-up ADR defines the DDL / storage.
- **SQL-engine wiring** — engines currently don't consult
  these policies. A caller invokes them explicitly.
- **Hierarchical tenant trees** / parent-child scoping.
- **Mutation-side column security** — CLS filters READS
  only; writes bypass per the projection semantics.
- **Policy DSL** — the predicate enum is intentionally
  minimal; richer grammar is a follow-up.

## Compliance

- Row-level allow-all / always-deny / column-equals /
  column-not-equals.
- Row-level unknown-column errors.
- Column-level projection filters.
- Column-level allow-all returns full row.
- Tenant allows in-scope, rejects out-of-scope.
- Tenant null column errors.
- Composite policy composes row-level + tenant.
- Composite policy denies on always-deny row predicate.
- 9 unit tests.
