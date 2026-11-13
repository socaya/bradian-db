# Bradian G44 executed verification — 13 November 2026

## Result

**658 passed, 0 failed, 0 ignored**, including **403 relational tests**, across
all Rust workspace features. Workspace compilation, strict Clippy across all targets
and features, and Studio typecheck/build passed. Baseline:
`7fd53f115cc77b6a17340740c10d68fc86881c52`.

Environment: Linux x86_64, Rust/Cargo 1.100.0 stable, Node 24.22.1, npm 11.9.0.
No dependencies, lockfiles or persistent formats changed. Automatic CI remains
manual-only. Existing unused manifest dependency/field advisories remain.

| Executed command | Result |
|---|---|
| `cargo check --workspace --all-features --locked` | Passed |
| `cargo test --workspace --all-features --locked --no-fail-fast` | 658 passed; no failures or ignored tests |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed |
| `npm run typecheck` in `apps/dap-ui` | Passed |
| `npm run build` in `apps/dap-ui` | Passed |

## Delivered and verified

G44 adds typed historical ORDER BY, NULLS FIRST/LAST and bounded LIMIT/OFFSET.
Column binding completes before row access; sorting/windowing precede projection.
Explicit sort ties and unordered windows use logical primary-key ordering. Bare
SELECT and older SQL engines preserve earlier behavior.

Nine SQL integrations verify signed numeric, fixed-scale decimal, text, boolean
and NULL ordering; unprojected sort columns; multi-key direction; deterministic
pages within one snapshot while another session commits; exact-PK windows; AS OF
schema/default binding; staged inserts, updates and deletes; savepoint rollback;
invalid grammar, names and numeric bounds; failed-state recovery; scan conflicts
including LIMIT 0; preservation of independent point-read transactions; exact
16-key and 10000-input-row boundaries; checkpoint/verified archive restore; and
older-engine compatibility. Two native tests verify timestamp/UUID ordering and
composite primary-key tie order independent of physical column layout.

The input-budget test builds 10000 rows, verifies the boundary, then adds one row
and proves that ORDER BY/LIMIT/OFFSET reject rather than return a truncated subset.
An invalid-binding test proves failed parsing does not accidentally record a full
scan and block unrelated validated writes. All earlier analytical, relational,
HTTP, migration, backup, corruption and recovery suites pass.

## Limits

This is single-table, in-memory query processing, not index-seek, streaming,
keyset or distributed pagination. The existing domain scan materializes records
before the 10000-row input guard; the guard bounds sorting/window work, not scan
memory. Text uses binary UTF-8 ordering without locale collation or normalization.
OFFSET pages across separate changing snapshots may shift. General predicates,
joins, expression/ordinal/alias ordering and wider SQL remain incomplete.

No browser interaction, external SQL-server differential qualification, physical
power-loss, disk-full or cross-platform recovery tests were run for this increment.
See the [gap register](BRADIAN_IMPLEMENTATION_GAPS.md).
