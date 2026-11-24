# ADR 0253 — Fault injection on user and DDL journals

## Status

Accepted.

## Context

ADR 0252 landed byte-level fault-injection utilities and
applied them to the `RowStore` WAL to pin its recovery
contract. Two other journals built on the same `RowStore` —
`PersistentUserCatalog` and `PersistentCatalog` — were not
yet covered by the same set of claims. This slice closes
that gap so P0-5 evidence exists for every journal we
currently maintain.

## Decision

Apply the ADR 0252 primitives to each journal's `rows.wal`
with the same four patterns:

1. **Clean reopen** reproduces the committed state (no
   mutation).
2. **Flip a middle byte** — must surface an explicit error
   (`Storage` / `Corruption` / `Encoding`) and never
   silently expose the corrupted record.
3. **Truncate the tail by one byte** — must either drop the
   torn frame and reopen with the earlier prefix intact, OR
   return an explicit error. Earlier events (e.g. the first
   `CreateUser`, the first `CreateDatabase`) must remain
   readable in the drop-and-reopen case.
4. **Zero the WAL magic** — must reject the file.

A fifth control-case is covered for each journal: **deleting
the WAL entirely** must not be a corruption event — the
journal reopens empty, so the operator can recreate from
scratch after a known reset.

## Consequences

- P0-5 evidence now spans all three journal types written by
  `RowStore`: row-store WAL (ADR 0252), user-catalog journal
  (this ADR), and DDL-catalog journal (this ADR).
- The user and catalog journals share the `RowStore`
  infrastructure, so these tests also pin that the higher
  journals correctly propagate `Storage` and `Corruption`
  errors through their own error variants rather than
  swallowing them.
- 5 cases in `tests/fault_injection_user_journal.rs` and
  5 cases in `tests/fault_injection_catalog_journal.rs`.
- Scope limits unchanged from ADR 0252: no runtime
  `FaultPolicy` trait yet, no scripted-workload property
  harness. Both remain future slices.
