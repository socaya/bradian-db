# Bradian G43 executed verification — 13 November 2026

## Result

**647 passed, 0 failed, 0 ignored**, including **392 relational tests**, across
all Rust workspace features. Workspace compilation, strict Clippy across all targets
and features, and Studio typecheck/build passed. Baseline:
`a5df2e37dcb1c38e7e9d2d9cc77bb32504ea970c`.

Environment: Linux x86_64, Rust/Cargo 1.100.0 stable, Node 24.22.1, npm 11.9.0.
No dependencies, lockfiles or persistent formats changed. Automatic CI remains
manual-only. Existing unused manifest dependency/field advisories remain.

| Executed command | Result |
|---|---|
| `cargo check --workspace --all-features --locked` | Passed |
| `cargo test --workspace --all-features --locked --no-fail-fast` | 647 passed; no failures or ignored tests |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed |
| `npm run typecheck` in `apps/dap-ui` | Passed |
| `npm run build` in `apps/dap-ui` | Passed |

## Delivered and verified

G43 adds checked expressions to exact-primary-key historical SQL UPDATE: numeric
arithmetic, signs, parentheses, same-type column references and standalone DEFAULT.
All assignments read the original row and the complete replacement is validated
before staging. Numeric overflow, division by zero and inexact fixed-scale decimal
results are errors. Earlier SQL engines retain their literal-only grammar.

Eleven SQL integrations verify precedence and associativity, assignment swapping,
original-row semantics, signed integer minima and overflow, integer division,
38-digit decimal multiplication/division, NULL propagation, DEFAULT and required
constraints, invalid binding/grammar/comments, missing-row behavior, parser budget
boundaries, whole-statement/transaction rollback, savepoint error recovery, opt-in
read validation, snapshot pins, historical reads, checkpoint/reopen, verified archive
restore and older-engine compatibility. A 256-case decimal corpus compares all four
operators against an independent small rational reference.

The new expression lexer is selected only for historical sessions. Existing negative
INSERT values and predicates remain accepted, including i64 minima, while comments
and unsupported arithmetic outside UPDATE fail explicitly. The full earlier
analytical, relational, HTTP, migration, backup, corruption and recovery suites pass.

## Limits

The SQL profile remains bounded: UPDATE requires a full primary key, operands must
match the target type, decimal operations require exact declared-scale results, and
SELECT/INSERT/WHERE expressions, CASE, functions, joins and subqueries remain open.
No implicit numeric widening or rounding is claimed. Secondary/foreign-key
constraints, full isolation-level qualification, concurrent online DDL, enterprise
security, HTAP and distributed SQL remain separate requirements.

No browser interaction, external SQL-server differential qualification, physical
power-loss, disk-full or cross-platform recovery tests were run for this increment.
See the [gap register](BRADIAN_IMPLEMENTATION_GAPS.md).
