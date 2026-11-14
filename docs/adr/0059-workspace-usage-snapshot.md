# ADR 0059 — Observational workspace usage snapshot (G1-MEMORY observability)

**Status:** Accepted for operator diagnostics across the decoded state
of a `PersistentCatalog`, a `MultiTableStore` and an opt-in `MvccStore`.
Gate 1 remains active. This ADR introduces **no admission**, no new
`ResourceError` variant and no persistent format change.

## Context

Nine admission policies ship with per-engine accessors (`resource_usage`,
`registered_table_count`, `MvccStats`). Operators who have composed
more than one engine (for example a `DomainSqlEngine` plus an opt-in
`MvccStore` plus a stand-alone `PersistentCatalog`) have no single
call that returns a combined decoded-state view.

The gaps analysis §9 and the implementation plan §6.1 list this as a
code-only follow-up: an observational aggregator that reports — but
does not enforce — the combined counts. It complements the admission
ADRs (0043–0058) rather than extending them.

## Decision

Add `workspace_usage::WorkspaceUsage`:

```rust
pub struct WorkspaceUsage {
    pub catalog: CatalogUsage,
    pub domain_tables: Option<usize>,
    pub mvcc_distinct_keys: Option<usize>,
    pub mvcc_total_versions: Option<usize>,
}

impl WorkspaceUsage {
    pub fn for_catalog(catalog: &PersistentCatalog) -> Self;
    pub fn with_domain(self, domain: &MultiTableStore) -> Self;
    pub fn with_mvcc(self, mvcc: &MvccStore) -> Self;
}
```

`Display` prints a single human-readable line suitable for operator
logs. Each `with_*` method takes **one atomic reading** from its
source engine under the engine's own lock, then returns the updated
snapshot by value. The snapshot does not retain references and does
not track live changes.

## Invariants

1. **Non-admitting.** No `ResourceError` can arise from constructing
   a `WorkspaceUsage`. Admission remains the responsibility of each
   engine's `open_with_limits` / commit paths.
2. **Frozen past view.** A snapshot captured at time `t` reports
   counts as of `t`; subsequent mutations in any engine do not
   propagate.
3. **No lock escalation.** Each `with_*` call releases its engine
   lock before returning.
4. **Reverse-dependency safe.** `runtime_limits.rs` keeps no engine
   imports; `workspace_usage.rs` lives above it and imports from
   `mvcc`, `persistent_catalog` and `transaction_domain` directly.

## Not covered

- A broader subset of engines (durable MVCC, typed MVCC, history SQL).
  They expose their own stats accessors; extending `WorkspaceUsage`
  to compose them is additive and can land later without changing
  the base shape.
- A persistent telemetry channel. BRD-OPS-001 "Studio metrics
  integration" remains an open item; this ADR explicitly does not
  implement it.
- A global process-wide RSS or cgroup integration — a §13
  owner-decision item in the comprehensive implementation plan.

## No persistent format, wire protocol, SQL grammar or dependency
change. No admission ADR is amended.
