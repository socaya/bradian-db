# ADR 0187 — FOREIGN KEY enforcement across session engines (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-22.** Mirrors ADR 0184 /
ADR 0185 FK enforcement from `SqlEngine` into
`DomainSqlEngine`, `HistorySqlEngine`, and `MvccSqlEngine`.

## 1. Scope

- New `sql::TxFkRowAccess` trait abstracting the row-
  access surface (`fk_scan` / `fk_delete` / `fk_replace`)
  that FK enforcement needs. Impls for the three session
  transaction types:
  - `transaction_domain::MultiTableTransaction<'_>`
  - `durable_typed::DurableTypedTransaction<'_>`
  - `typed_mvcc::TypedMvccTransaction<'_>`
- New shared helpers in `sql.rs`:
  - `fk_check_child_row_tx(catalog, table, row, tx)` —
    parent-exists check at INSERT / UPDATE.
  - `fk_apply_parent_delete_tx(catalog, parent, pk, tx)`
    — RESTRICT / CASCADE / SET NULL dispatch at DELETE.
  Both read / write through the supplied tx so the
  surrounding session's pending state is visible.
- Call-site wiring:
  - `DomainSqlSession::execute_inner` INSERT →
    `fk_check_child_row_tx` → UNIQUE → `tx.insert`.
    UPDATE → mutated-row check. DELETE →
    `fk_apply_parent_delete_tx` → `tx.delete`.
  - `HistorySqlSession::execute_inner` mirrors the same
    three sites.
  - `MvccSqlSession::execute_inner` mirrors the same
    three sites.

## 2. Out of scope

- **PK UPDATE cascading.** All session engines reject
  PK UPDATE; `on_update` remains inert.
- **Cross-engine FKs.** A table referenced by an FK must
  live in the same engine's visible catalog + tx. The
  PK-space validation at `add_foreign_key` already
  refuses cross-database FKs.
- **Deferrable / initially deferred FKs.** Immediate
  only.
- **Named `CONSTRAINT fk_name FOREIGN KEY ...` syntax.**
  Auto-generated names only.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/foreign_key_session_engines.rs`
(11 tests, all green):

DomainSqlEngine (5):
1. `domain_insert_missing_parent_is_rejected`.
2. `domain_insert_sees_pending_parent_in_same_tx` —
   proves `tx.scan` visibility.
3. `domain_parent_delete_restrict`.
4. `domain_parent_delete_cascade_removes_children`.
5. `domain_parent_delete_setnull`.

HistorySqlEngine (3):
6. `history_insert_missing_parent_is_rejected`.
7. `history_parent_delete_restrict`.
8. `history_parent_delete_cascade`.

MvccSqlEngine (3):
9. `mvcc_insert_missing_parent_is_rejected`.
10. `mvcc_parent_delete_restrict`.
11. `mvcc_parent_delete_cascade`.

Clippy clean on `-D warnings`. The ADR 0184 /
ADR 0185 SqlEngine test suites continue to pass.

## 4. Roadmap impact

BRD-REL-002 FK enforcement coverage after this ADR:

| Engine | INSERT | UPDATE | DELETE actions |
|---|---|---|---|
| `SqlEngine` | ADR 0184 | ADR 0184 | ADR 0184/0185 |
| `DomainSqlEngine` | **ADR 0187** | **ADR 0187** | **ADR 0187** |
| `HistorySqlEngine` | **ADR 0187** | **ADR 0187** | **ADR 0187** |
| `MvccSqlEngine` | **ADR 0187** | **ADR 0187** | **ADR 0187** |

BRD-REL-002 UNIQUE + CHECK + FK across every Bradian
SQL surface is now complete at Gate 1 scope.
