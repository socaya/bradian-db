# Bradian G39 executed verification — 13 November 2026

## Result

**600 passed, 0 failed, 0 ignored**, across all Rust workspace features, including
**345 relational tests**. Workspace compilation, strict Clippy across all targets and
features, and Studio typecheck/build passed. Baseline: `be7ea6205b27d1eaca6f78f0023c744c973f9482`.

Environment: Linux x86_64, Rust/Cargo 1.100.0 stable, Node 24.22.1 and npm 11.9.0.
Cargo uses the existing lockfile with `--locked`. No dependencies or persistent row,
page or WAL envelope formats changed. Automatic CI remains manual-only.

| Executed command | Result |
|---|---|
| `cargo check --workspace --all-features --locked` | Passed |
| `cargo test --workspace --all-features --locked --no-fail-fast` | 600 passed; no failed/ignored tests |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed |
| `npm run typecheck` in `apps/dap-ui` | Passed |
| `npm run build` in `apps/dap-ui` | Passed |
| `git diff --cached --check` | Passed before publication |

Cargo continues to emit the existing unused manifest dependency/field advisories.
No failing compiler or Clippy diagnostics remain in the recorded run.

## Implementation and compatibility

G39 relaxes a non-key NOT NULL column through G36's exclusive cross-WAL coordinator.
It also permits historical SQL INSERT to omit nullable fields, assigning NULL;
required columns remain mandatory. Row/schema history is preserved, new writes use
the new epoch, and AS OF retains the original schema and values.

A distinct catalog proposal command provides the new compatibility rule. The G33
proposal API deliberately keeps its earlier strict contract. Existing G36 additions
continue using the original command. Older readers fail closed on a relaxation
intent or its new catalog event rather than reinterpret the schema.

## New regression evidence

Seven SQL integrations exercise persisted NULL writes and historical DESCRIBE,
omitted nullable columns, missing/duplicate/unknown field rollback, invalid DDL and
no-op rejection, mixed additions/relaxations and multi-table commits, backup/restore,
retention, original row/pin preservation and retirement of old inactive intents.
Two planner tests prove monotone projection and reject PK relaxation/narrowing.
One catalog test preserves the G33 additions-only contract.

Two recovery tests inject failures at all four durable boundaries and truncate
sixteen relaxation transition frames. Reopen rolls forward only valid interrupted
states, repairs partial tails, preserves historical NOT NULL semantics and remains
idempotent. The complete earlier recovery, corruption, migration, analytical,
security-primitive and HTTP suites also pass.

## Limits

This is exclusive local DDL, not concurrent online DDL or full SQL conformance.
Non-NULL default expressions, DEFAULT VALUES, SET NOT NULL/backfill, DROP/RENAME/type
changes, serializable isolation, production HTAP/distributed SQL, enterprise security
and physical-device/cross-platform recovery qualification remain open in the
[gap register](BRADIAN_IMPLEMENTATION_GAPS.md). No browser interaction, disk-full or
physical power-loss tests were run for this increment.
