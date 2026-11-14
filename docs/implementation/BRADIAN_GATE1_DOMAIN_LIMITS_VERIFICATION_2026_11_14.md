# Master Gate 1 — Decoded domain admission verification

14 November 2026. Baseline: `93fd9a5`.
Stable task G1-MEMORY in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**827 Rust tests passed, 0 failed, 0 ignored**. Workspace compilation and
Clippy (`-p bradian-relational --all-targets -- -D warnings`) passed.
CI remains manual-only. No dependency, lockfile, persistent format, SQL
grammar or default isolation changed.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-relational` | Passed |
| `cargo test --workspace` | 827 passed, 0 failed, 0 ignored |
| `cargo test -p bradian-relational --test domain_limits` | 4 passed |
| `cargo clippy -p bradian-relational --all-targets -- -D warnings` | Passed |

## Implemented and exercised

`DomainLimits { max_tables }` added to `runtime_limits.rs` with a default
of 65_536 per domain. `ResourceError::DomainLimit { resource, limit }`
is distinct from storage, corruption and catalog admission errors.

`MultiTableStore::open_with_limits(root, limits)` accepts an explicit
policy; `open` retains its signature and uses the default.
`resource_limits()` and `registered_table_count()` expose the configured
policy and current count.

Admission points:
1. `open()` — after each `tables.insert(...)` succeeds during replay, the
   decoded count is admitted. A refusal returns `ResourceError::DomainLimit`
   and the WAL is byte-identical to pre-refusal state.
2. `register_table()` — admission runs on `registered.len() + 1` **before**
   any WAL append. A refusal leaves the live map, WAL and catalog
   unchanged.

Four new tests cover (1) live registration refusal with WAL preservation,
(2) idempotent re-registration of the same table not charging additional
capacity, (3) tighter reopen limits refusing without modifying the WAL,
(4) default limits admitting an ordinary 8-table domain.

## Remaining scope

- Cross-engine decoded-state totals (`MultiTableStore` + `DurableMvccStore`
  + `PersistentCatalog`).
- Global process-wide decoded-state ceiling — owner decision from the
  comprehensive implementation plan §13.
- Row-value decoded state inside a transaction (already bounded by
  staging and single-table snapshot admission, ADRs 0042/0044).

See [ADR 0056](../adr/0056-decoded-domain-admission.md) and the
[fixed roadmap](BRADIAN_MASTER_ROADMAP.md).
