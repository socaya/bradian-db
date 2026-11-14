# Master Gate 1 — Decoded catalog admission verification

14 November 2026. Baseline: `b664504`.
Stable task G1-MEMORY in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**797 Rust tests passed, 0 failed, 0 ignored**. Workspace compilation and
Clippy (`-p bradian-relational --all-targets -- -D warnings`) passed. CI
remains manual-only. No dependency, lockfile, persistent format, SQL
grammar or default isolation changed.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-relational` | Passed |
| `cargo test --workspace` | 797 passed, 0 failed, 0 ignored |
| `cargo test -p bradian-relational --test catalog_limits` | 5 passed |
| `cargo clippy -p bradian-relational --all-targets -- -D warnings` | Passed |

## Implemented and exercised

`CatalogLimits { max_databases, max_schemas, max_tables, max_revisions,
max_event_bytes }` and `CatalogUsage` were added to `runtime_limits.rs`
with sensible defaults (1024 / 4096 / 65_536 / 1024 / 64 KiB).
`ResourceError::CatalogLimit { resource, limit }` is distinct from catalog
semantic errors, storage errors, JSON encoding and corruption.

`PersistentCatalog::open_with_limits(path, limits)` accepts an explicit
policy; `open` retains its signature and uses the default. `resource_limits()`
and `resource_usage()` expose the configured policy and current counts.

Admission points:

1. `commit()` — after the tentative catalog/revisions are updated by the
   replay validator, check decoded counts and the serialized event bytes
   before any WAL append. A refusal leaves the live catalog, revisions
   and WAL exactly as they were.
2. `open()` — each journaled event's stored bytes are admitted before
   decode; the decoded counts are checked after each `event.apply`.
   A refusal leaves the WAL byte-identical and never publishes a partial
   catalog.

Five new tests cover (1) live refusal of oversized database count with
state preservation, (2) separate schema and table refusals with usage
accounting, (3) tighter reopen limits refused without modifying the WAL,
(4) oversized event byte refusal before WAL append, (5) default limits
admit an ordinary working catalog. Existing SQL, analytical, recovery,
backup/migration and admission suites still pass under the default
policies.

## Remaining scope

- Per-table column, FK and unique-key counts beyond the existing RowStore
  inline-record limit on serialized event bytes.
- Decoded catalog state across multiple domains/engines.
- Global process-wide decoded-state totals and a versioned catalog
  compactor (the journal is still append-only).

See [ADR 0054](../adr/0054-decoded-catalog-admission.md) and the
[fixed roadmap](BRADIAN_MASTER_ROADMAP.md).
