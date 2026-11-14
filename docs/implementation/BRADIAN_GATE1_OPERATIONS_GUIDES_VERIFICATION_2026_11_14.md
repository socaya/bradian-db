# Master Gate 1 — Operations guides for 0051/0054/0056/0058 verification

14 November 2026. Baseline: `6fb2a42`.
Hygiene slice; docs-only. Gate 1 remains active.

## Executed result

**848 Rust tests passed, 0 failed, 0 ignored** (unchanged by docs-only
slice). No code change.

| Executed check | Result |
|---|---|
| `cargo test --workspace` | 848 passed (unchanged) |
| Link validation on `docs/operations/README.md` | All links resolve |

## Action

Four shipped admission policies — directory listings (ADR 0051), decoded
persistent-catalog state (0054), decoded multi-table-domain state
(0056) and in-memory MvccStore versions (0058) — had ADRs and
verification reports but no dedicated operator-facing guide. The
previous `operations/README.md` (Slice J) listed them in a "without
guide" table; operators who wanted to tune them had to read the ADRs.

Four new guides match the existing house style (context paragraph,
defaults table, native configuration example, refusal and recovery
behaviour, example tightening, remaining scope, cited ADR and
verification):

- `docs/operations/DIRECTORY_LISTING_LIMITS.md` — ADR 0051.
- `docs/operations/CATALOG_LIMITS.md` — ADR 0054.
- `docs/operations/DOMAIN_LIMITS.md` — ADR 0056.
- `docs/operations/MVCC_STORE_LIMITS.md` — ADR 0058.

`operations/README.md` is updated so the main guides table now
enumerates 15 entries (up from 11); the "without dedicated guide"
section is retired. The four new guides give operators the same
single-pane-of-glass view the earlier ones provided.

## Remaining scope

- Range `ORDER BY` corpus entries remain noted in
  `BRADIAN_SQL_SUBSET.md` §14 as not yet covered — the parser rejects
  range predicates, so nothing can be added positively.
- §13 owner-decision items in the
  [comprehensive implementation plan](BRADIAN_COMPREHENSIVE_IMPLEMENTATION_PLAN.md)
  are unchanged.
