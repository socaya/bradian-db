# ADR 0160 — Session-engine driver adapters (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-18.** Extends ADR 0158's
`SqlEngineAdapter` to all three session-based engines
(`HistorySqlEngine`, `DomainSqlEngine`, `MvccSqlEngine`).
Every SQL engine in the codebase is now reachable
through a `dyn Driver`.

## 1. Scope

New module `driver_adapter_sessions` with three adapters:

- `HistorySqlEngineAdapter { engine: Arc<HistorySqlEngine> }`
- `DomainSqlEngineAdapter { engine: Arc<DomainSqlEngine> }`
- `MvccSqlEngineAdapter { engine: Arc<MvccSqlEngine> }`

All three `impl Driver` with **autocommit semantics**:
each `execute(sql)` opens a fresh `with_session` and
wraps the caller's statement in an implicit BEGIN +
COMMIT. Transaction commands (`BEGIN` / `COMMIT` /
`ROLLBACK` / `SAVEPOINT` / `RELEASE`) are detected and
passed through unwrapped, since multi-statement
transactions can't span per-call sessions.

Shared `translate_result` / `translate_error` helpers
mirror ADR 0158's mapping between `SqlResultSet` /
`SqlError` and `DriverResponse` / `DriverError`.

## 2. Out of scope (follow-ups)

- **Session-aware driver trait** — the current
  `Driver::execute(sql)` can't hold a BEGIN/COMMIT
  session across calls. A future `SessionDriver` trait
  with `begin_session(&self) -> SessionHandle` fixes
  this for clients that need multi-statement
  transactions over the wire.
- **Prepared-statement execution** — ADR 0161 companion.
- **Network transport** — ADR 0162 companion.

## 3. Compliance

5 unit tests in `driver_adapter_sessions::tests`:

- Domain autocommit INSERT + SELECT round-trip.
- Mvcc autocommit DML behind `Box<dyn Driver>`.
- Transaction-command pass-through semantics.
- Domain adapter clone.
- `is_transaction_command` detects BEGIN/COMMIT/
  ROLLBACK/SAVEPOINT/RELEASE in any case + trim.

## 4. Honest status

**BRD-WIRE-001** gains session-engine coverage. The
remaining open items (prepared statements, network
transport, TLS) are the subject of ADR 0161–0163.
