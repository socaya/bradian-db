# ADR 0149 — DML byte-budget charges (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-17.** Extends ADR 0147
(non-SELECT row charges) and ADR 0148 (SELECT byte
charges) to charge `max_bytes_materialized` on INSERT /
UPDATE / DELETE / TRUNCATE across all four SQL engines.
Same additive-API pattern.

## 1. Scope

All four engines, four statement kinds now charge
`budget.charge_bytes()` alongside `charge_rows()`:

| Kind | Bytes charged |
|---|---|
| INSERT | `row_bytes(inserted_row)` |
| UPDATE | `row_bytes(replacement_row)` if hit, else 0 |
| DELETE | `row_bytes(existing_row)` if hit, else 0 |
| TRUNCATE (SqlEngine only) | `rows_bytes(scanned_rows)` |

Reuses the `row_bytes` / `rows_bytes` helpers shipped
in ADR 0148.

## 2. Out of scope (follow-ups)

- **Iteration charges** — `max_iterations` still
  reserved.
- **Mid-scan cancellation** — breach detection remains
  at check points only.
- **DDL charges** — CREATE / DROP / ALTER don't
  consult the budget.
- **Multi-row INSERT** — parser accepts one row;
  when multi-row lands, charges scale linearly.
- **Pre-mutation prediction** — the charge happens
  AFTER the mutation succeeds. For truly bounded
  staging a future ADR can preflight a `would_exceed`
  check before copying the row.

## 3. Compliance

Updated four engine test files. Totals now:

- `sql_engine_budget.rs` — 18 tests (added 5: insert
  bytes, insert over byte budget, update bytes, delete
  bytes, truncate sum).
- `history_sql_budget.rs` — 10 tests (added 1:
  update over byte budget).
- `domain_sql_budget.rs` — 10 tests (added 1: insert
  over byte budget).
- `mvcc_sql_budget.rs` — 10 tests (added 1: insert
  over byte budget).

Per-engine exact byte-count assertions on SqlEngine
(known seed) prove the `row_bytes` wiring. Session
engines verify the breach path without over-fitting
on specific seed bytes.

## 4. Honest status

**BRD-RUNTIME-003** now has row + byte coverage on
every DML path across every SQL engine. Iteration
charges, mid-scan cancellation, DDL charges, and
multi-row INSERT remain OPEN per §2.
