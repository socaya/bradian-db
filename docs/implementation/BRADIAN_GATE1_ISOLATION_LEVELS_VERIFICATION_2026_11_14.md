# Master Gate 1 — Isolation levels written statement verification

14 November 2026. Baseline: `76f89c5`.
Stable task G1-REFERENCE in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**834 Rust tests passed, 0 failed, 0 ignored**. No code change; this
slice adds a reader-facing reference document only.

| Executed check | Result |
|---|---|
| `cargo test -p bradian-relational --test read_validation` | 7 passed |
| `cargo test -p bradian-relational --test read_validation_sql` | 6 passed |
| Reference citation count | 12 test names cited, 12 verified to exist |

## Implemented and exercised

`docs/reference/BRADIAN_ISOLATION_LEVELS.md` publishes a written
statement of Bradian's two concurrency modes (snapshot default,
bounded opt-in read-validation) and their behaviour against each
classical isolation anomaly:

- Dirty read — not possible in either mode.
- Non-repeatable read — not possible.
- Phantom read — not possible; scan observations revalidate.
- Lost update — prevented.
- Write skew — possible in snapshot, prevented under read-validation
  within its 4096-key budget, with whole-domain fallback above.
- Read-only anomalies — validator catches them even on read-only
  transactions.
- ABA observation — detected under read-validation.

Every row cites the owning existing test in `tests/read_validation.rs`
and `tests/read_validation_sql.rs`. Savepoint interaction, AS OF
interaction and the all-twenty-interleaving exhaustive schedule are
documented with their owning tests. §7 of the reference lists the
claims this document explicitly does NOT make, including external
SQL-standard SERIALIZABLE mapping and distributed isolation — those
remain open under G1-REFERENCE.

## Remaining scope

- External SQL-standard isolation labels (READ UNCOMMITTED / READ
  COMMITTED / REPEATABLE READ / SERIALIZABLE) mapped to Bradian
  behaviour by a published external reference suite.
- Distributed/cross-process isolation statements (blocked on Gate 5).
- Formal proof or certification; this is an executable-evidence
  document, not a mathematical proof.

See [ADR 0037](../adr/0037-bounded-transaction-read-validation.md),
[ADR 0036](../adr/0036-bounded-history-transaction-savepoints.md) and
the [comprehensive implementation plan](BRADIAN_COMPREHENSIVE_IMPLEMENTATION_PLAN.md)
§4.4.
