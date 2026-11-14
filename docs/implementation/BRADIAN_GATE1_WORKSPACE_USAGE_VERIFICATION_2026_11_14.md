# Master Gate 1 — Workspace usage snapshot verification

14 November 2026. Baseline: `39f6e6c`.
Observability slice; **non-admitting**. Gate 1 remains active.

## Executed result

**860 Rust tests passed, 0 failed, 0 ignored.** Clippy clean on
`bradian-relational --all-targets`. No persistent format or SQL
grammar change. No new `ResourceError` variant.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-relational` | Passed |
| `cargo test --workspace` | 860 passed, 0 failed |
| `cargo test -p bradian-relational --test workspace_usage` | 5 passed |
| `cargo clippy -p bradian-relational --all-targets -- -D warnings` | Passed |

## Finding and action

The gaps analysis §9 flagged "observational `resource_usage()`
aggregator across catalog + domain + MVCC" as a code-only follow-up.
Nine admission policies ship with per-engine usage accessors, but
no single call combined them. Operators who had composed multiple
engines had to read each one separately.

Change:

- New `bradian_relational::workspace_usage` module with
  `WorkspaceUsage` (catalog counts always present; `domain_tables`,
  `mvcc_distinct_keys`, `mvcc_total_versions` each `Option<usize>`).
- Builder methods: `for_catalog`, `with_domain`, `with_mvcc`. Each
  takes exactly one atomic reading from the source engine.
- `Display` emits a one-line human-readable form for operator logs.
- Added to `lib.rs` as a top-level public module.
- Five new tests in `tests/workspace_usage.rs` prove:
  1. Catalog-only snapshot reports current counts;
  2. `with_domain` adds the registered-table count without touching
     the catalog fields;
  3. `with_mvcc` adds retained keys and versions;
  4. Full snapshot combines all three engines and `Display` includes
     each attached engine on one line;
  5. **A snapshot is a frozen past view** — mutating every engine
     after taking a snapshot does not alter the snapshot's values.

## Properties the slice guarantees

- **Non-admitting.** No `ResourceError` can arise from constructing
  a snapshot.
- **Frozen view.** Snapshot does not retain references or track live
  changes.
- **No lock escalation.** Each `with_*` releases its engine lock
  before returning.
- **Reverse-dependency safe.** `runtime_limits.rs` remains free of
  engine imports; `workspace_usage.rs` lives above it.

## Remaining scope

- `WorkspaceUsage` currently covers `PersistentCatalog` +
  `MultiTableStore` + `MvccStore`. `DurableMvccStore`,
  `TypedMvccStore` and `HistorySqlEngine` expose their own stats
  accessors; extending the snapshot to compose them is additive and
  can land later without changing the base shape.
- BRD-OPS-001 Studio metrics integration remains open.
- Global process-wide RSS / cgroup integration remains a §13
  owner-decision item.

See [ADR 0059](../adr/0059-workspace-usage-snapshot.md),
[comprehensive gaps analysis](BRADIAN_COMPREHENSIVE_GAPS_ANALYSIS.md) §9,
[fixed roadmap](BRADIAN_MASTER_ROADMAP.md).
