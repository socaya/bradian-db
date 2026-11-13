# Bradian G40 executed verification — 13 November 2026

## Result

**611 passed, 0 failed, 0 ignored**, including **356 relational tests**, across
all Rust workspace features. Workspace compilation, strict Clippy across all targets
and features, and Studio typecheck/build passed. Baseline:
`204ba71f2b344740233ab7433939f18fc6bbfc1c`.

Environment: Linux x86_64, Rust/Cargo 1.100.0 stable, Node 24.22.1, npm 11.9.0.
Existing lockfiles and dependencies were preserved. Automatic CI remains manual-only.

| Executed command | Result |
|---|---|
| `cargo check --workspace --all-features --locked` | Passed |
| `cargo test --workspace --all-features --locked --no-fail-fast` | 611 passed; no failures or ignored tests |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed |
| `npm run typecheck` in `apps/dap-ui` | Passed |
| `npm run build` in `apps/dap-ui` | Passed |

Existing unused manifest dependency/field advisories remain. No failing compiler or
Clippy diagnostics remain in these runs.

## Delivered and verified

G40 adds immutable typed constant defaults to historical ADD COLUMN, including
required columns. Old rows project defaults without rewriting original payloads.
INSERT omissions and explicit DEFAULT use declared values; explicit NULL retains
its distinct semantics. Historical DESCRIBE optionally reports defaults.

Seven new SQL integrations cover required/nullable defaults, clause order, escaped
text, booleans, maximum 38-digit decimals, integer limits, invalid definitions,
statement rollback, native API validation, bounded journal capacity, mixed schema
epochs, retention, checkpoint/reopen and verified backup/restore. Original row and
schema-pin bytes are checked unchanged. A serialization test preserves exact legacy
Table bytes when defaults are empty. A planner test proves immutable birth defaults
and the stricter old API contracts.

Two new recovery tests exercise all four interruption boundaries plus sixteen torn
frames, idempotent reopen and subsequent omitted-value INSERT. Forged version-1
default intents and mismatched activation versions fail before catalog repair. All
prior workspace recovery, migration, analytical and HTTP tests also pass.

## Limits

This increment is exclusive local DDL. It does not deliver concurrent online DDL,
SET/DROP DEFAULT, expressions, DEFAULT VALUES, existing-data SET NOT NULL/backfill,
DROP/RENAME/type changes, savepoints or serializable isolation. Enterprise transport,
identity, HTAP and distributed SQL remain incomplete. No browser interaction,
disk-full, physical power-loss or cross-platform qualification was run. See the
[gap register](BRADIAN_IMPLEMENTATION_GAPS.md).
