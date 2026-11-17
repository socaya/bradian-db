# ADR 0148 — SELECT byte-budget charges (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-17.** Extends ADR 0141
`QueryBudget::max_bytes_materialized` from a reserved
knob to an observable effect on real SQL execution.
Complements the row charges wired in
ADR 0143 / 0144 / 0145 / 0146 (SELECT) and ADR 0147 (DML).

## 1. Scope

Two new free functions in
`crates/bradian-relational/src/query_budget.rs`:

- `row_bytes(row: &[Value]) -> u64` — approximate
  materialized byte size of one row. Tracks the stored
  width of each `Value` variant (not Rust alignment /
  discriminant padding).
- `rows_bytes(rows: &[Vec<Value>]) -> u64` — sum across
  a row slice.

Variant size map:

| Variant | Bytes |
|---|---|
| `Null` | 1 |
| `Boolean` | 1 |
| `Int32` | 4 |
| `Int64` | 8 |
| `Decimal` | 16 |
| `TimestampTz` | 8 |
| `Uuid` | 16 |
| `String(s)` | `s.len()` |

Four SQL engines, SELECT path only — after the existing
`charge_rows(rows.len())` call, each engine now also
calls `charge_bytes(rows_bytes(&rows))`.

## 2. Out of scope (follow-ups)

- **DML bytes** — INSERT / UPDATE / DELETE charge rows
  (ADR 0147) but not bytes. Bytes would need a per-row
  size on the staging path; follow-up.
- **DictIndex32 / columnar segment bytes** — this
  primitive counts row-oriented bytes only.
- **Varchar-max vs. actual bytes** — Varchar(N) with a
  20-char value charges 20, not N. Future projection
  may charge the declared ceiling.
- **Overhead / framing bytes** — frame envelopes
  (ADR 0137) and chunking (ADR 0140) are a transport
  concern outside this budget.
- **Iteration charges** — `max_iterations` still
  reserved.

## 3. Compliance

New byte-focused tests across the four engine test
files (one per engine):

- `sql_engine_budget.rs` — exact byte count for a
  known seed (5 rows × 12 bytes = 60); over-budget
  returns `SqlError::Budget(BudgetExceeded::Bytes)`.
- `history_sql_budget.rs`, `domain_sql_budget.rs`,
  `mvcc_sql_budget.rs` — each has a
  `select_over_byte_budget_errors` test proving the
  SELECT path breaches on a 0-byte cap.

`query_budget::tests` adds three unit tests for
`row_bytes` / `rows_bytes`.

## 4. Honest status

**BRD-RUNTIME-003** now has row + byte coverage on
SELECT across all four engines. DML byte charges,
iteration charges, cancellation, DDL charges, and
multi-row INSERT remain OPEN per §2.
