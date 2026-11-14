# Master Gate 1 — DomainSqlEngine admission propagation verification

14 November 2026. Baseline: `a94aefa`.
Stable task G1-MEMORY in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**855 Rust tests passed, 0 failed, 0 ignored**. Clippy clean on
`bradian-relational --all-targets`. No persistent format or SQL grammar
change.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-relational` | Passed |
| `cargo test --workspace` | 855 passed, 0 failed |
| `cargo test -p bradian-relational --test transitive_admission` | 7 passed (+3 new) |
| `cargo clippy -p bradian-relational --all-targets -- -D warnings` | Passed |

## Finding and action

The gaps analysis §9 flagged `DomainSqlEngine::open_with_limits` as
one of the remaining code-only follow-ups: Slice L (commit `f5c0f6c`)
threaded limits through `TypedMvccStore` and `MvccSqlEngine` but left
`DomainSqlEngine` using defaults, with no caller tightening path.

Change:

- `DomainSqlEngine::open_with_limits(root, domain_limits, catalog_limits)`
  — new entry point that threads `DomainLimits` to
  `MultiTableStore::open_with_limits` and `CatalogLimits` to
  `PersistentCatalog::open_with_limits`. The default
  `DomainSqlEngine::open` delegates with both defaults so existing
  callers are unchanged.
- `tests/transitive_admission.rs` — three new integration tests:
  1. Tight `DomainLimits` refuses via `open_with_limits`.
  2. Tight `CatalogLimits` refuses via `open_with_limits` (`SqlError::Catalog`).
  3. Default `DomainSqlEngine::open` still admits an ordinary workload
     (regression guard).

## Remaining scope

- The comprehensive gaps analysis lists this as the last trivially
  code-only follow-up in the admission-propagation workstream.
- Cross-engine aggregate ceilings and global process RSS remain §13
  owner-decision items.

See [ADR 0054](../adr/0054-decoded-catalog-admission.md),
[ADR 0056](../adr/0056-decoded-domain-admission.md),
[comprehensive gaps analysis](BRADIAN_COMPREHENSIVE_GAPS_ANALYSIS.md) §9,
[fixed roadmap](BRADIAN_MASTER_ROADMAP.md).
