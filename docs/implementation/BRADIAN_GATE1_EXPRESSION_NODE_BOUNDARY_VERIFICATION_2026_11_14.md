# Master Gate 1 — UPDATE expression 128-node boundary verification

14 November 2026. Baseline: `67bc9e1`.
Stable task G1-REFERENCE in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**847 Rust tests passed, 0 failed, 0 ignored**. Clippy clean. No
production code change; the engine's MAX_NODES=128 constant and
`spend()` accounting are exercised by two new corpus tests.

| Executed check | Result |
|---|---|
| `cargo test --workspace` | 847 passed |
| `cargo test -p bradian-relational --test sql_subset_corpus` | 35 passed (+2 new) |
| `cargo clippy -p bradian-relational --all-targets -- -D warnings` | Passed |

## Implemented and exercised

Two new tests exercise the published 128-node UPDATE expression budget:

- `update_expression_boundary_at_128_nodes_is_accepted` — constructs an
  expression at exactly 128 spends (`-balance` contributes 2; 63 `+ 1`
  terms contribute 2 each) and asserts it executes with the correct
  final value.
- `update_expression_at_129_nodes_is_refused` — one spend past the
  budget (`balance + 1 ×64`) is refused by the parser.

Node accounting (from `sql_expression.rs`):
- Each atom (literal, column reference, unary) spends 1 node.
- Each binary operator (`+`, `-`, `*`, `/`) spends 1 node.
- Parentheses spend 0 nodes but charge the 32-depth budget separately.

Reference `docs/reference/BRADIAN_SQL_SUBSET.md` §9 adds the two new
rows; §14 is tightened to remove the 128-node bullet.

## Remaining scope

Range ORDER BY cases and `TimestampTz` / `Uuid` sort behaviour remain
the next corpus extensions. External SQL-92/99/2016 reference-suite
compliance remains the open G1-REFERENCE acceptance bar.

See [ADR 0055](../adr/0055-sql-subset-conformance-corpus.md).
