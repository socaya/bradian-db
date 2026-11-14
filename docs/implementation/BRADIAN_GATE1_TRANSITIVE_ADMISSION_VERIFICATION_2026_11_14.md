# Master Gate 1 — Transitive admission propagation verification

14 November 2026. Baseline: `888371d`.
Stable task G1-MEMORY in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**852 Rust tests passed, 0 failed, 0 ignored**. Clippy clean on
`bradian-relational --all-targets`. No persistent format, wire protocol
or SQL grammar change.

| Executed check | Result |
|---|---|
| `cargo test --workspace` | 852 passed (+4 new) |
| `cargo test -p bradian-relational --test transitive_admission` | 4 passed |
| `cargo clippy -p bradian-relational --all-targets -- -D warnings` | Passed |

## Finding and action

The pre-slice session recap claimed that `TypedMvccStore` and
`MvccSqlEngine` were "transitively bounded" by upstream admission
policies. That was partly true — the defaults flow through — but
callers had no way to **tighten** them past the defaults. The default
`open` on each of the two higher engines constructed the underlying
`MvccStore` and `PersistentCatalog` with their default limits, with no
alternative entry point. Operators could not pass tight policies.

Changes:

- `TypedMvccStore::open_with_limits(root, catalog_tables, mvcc_limits)`
  — new entry point that threads `MvccStoreLimits` to the underlying
  `MvccStore::open_with_limits`. The default `open` delegates with
  `MvccStoreLimits::default()`.
- `MvccSqlEngine::open_with_limits(root, mvcc_limits, catalog_limits)`
  — new entry point that threads both `MvccStoreLimits` and
  `CatalogLimits` to the underlying `TypedMvccStore` and
  `PersistentCatalog`. The default `open` delegates with both defaults.
- ADR 0058 gains an amendment section describing the propagation
  contract.
- `tests/transitive_admission.rs` (new) — four integration tests prove:
  1. `TypedMvccStore::open_with_limits` with a tight `MvccStoreLimits`
     refuses during seeding.
  2. `MvccSqlEngine::open_with_limits` with a tight `CatalogLimits`
     refuses during catalog replay (`SqlError::Catalog` variant), WAL
     unchanged.
  3. `MvccSqlEngine::open_with_limits` with a tight `MvccStoreLimits`
     refuses during shared-domain seeding.
  4. Default `MvccSqlEngine::open` still admits an ordinary workload
     (regression guard).

## Remaining scope

- `DurableTypedStore::open_with_history_limits` already exists; it is
  not amended by this slice.
- `DomainSqlEngine` does not yet expose `open_with_limits` wrapping
  `MultiTableStore::open_with_limits`; adding it is a natural
  follow-up but not required for Gate 1.
- Cross-engine aggregate ceilings and process-wide RSS remain §13
  owner-decision items.

See [ADR 0058](../adr/0058-mvcc-store-admission.md),
[ADR 0054](../adr/0054-decoded-catalog-admission.md),
[ADR 0056](../adr/0056-decoded-domain-admission.md) and the
[fixed roadmap](BRADIAN_MASTER_ROADMAP.md).
