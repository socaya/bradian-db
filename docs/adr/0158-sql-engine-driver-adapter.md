# ADR 0158 — SqlEngine driver adapter (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-18.** Promotes ADR 0129's
`Driver` trait from a FOUNDATIONS primitive (only the
`NoopDriver` stub implemented it) to a PARTIAL
production surface: the base `SqlEngine` now has a
real adapter clients can depend on via `dyn Driver`.

## 1. Scope

New module `crates/bradian-relational/src/driver_adapter.rs`:

- `SqlEngineAdapter { engine: Arc<SqlEngine> }` —
  cheap to clone, all clones share one engine.
- `impl Driver for SqlEngineAdapter` — routes every
  `execute(sql)` to `engine.execute(sql)`.
- Result translation:
  - `SqlResultSet::Command { affected: 0 }` →
    `DriverResponse::Empty`.
  - `SqlResultSet::Command { affected: N }` →
    `DriverResponse::Affected { rows: N }`.
  - `SqlResultSet::Rows { columns, rows }` →
    `DriverResponse::Rows(Vec<DriverRow>)`.
- Error translation:
  - `SqlError::Resource` / `SqlError::Budget` →
    `DriverError::Resource(formatted_message)`.
  - All other `SqlError` → `DriverError::Sql(formatted_message)`.

Manual `impl Debug for SqlEngineAdapter` with
`debug_struct().finish_non_exhaustive()` since the
inner `SqlEngine` does not derive Debug.

## 2. Out of scope (follow-ups)

- **Session-based engines** — `HistorySqlEngine`,
  `DomainSqlEngine`, `MvccSqlEngine` each need an
  adapter that holds a persistent session handle and
  implements a session-aware driver trait.
- **Prepared-statement execution** via ADR 0139
  envelope — a future adapter method takes a
  `PreparedStatement` and binds parameters.
- **Network transport** — this adapter is in-process;
  wrapping it in ADR 0137 wire frames + a tokio
  listener is a separate ADR (bradian-serving).
- **Streaming row delivery** via ADR 0140 chunk
  assembler — the current `DriverResponse::Rows` is
  all-at-once.
- **TLS/mTLS** — transport concern, not adapter
  concern.

## 3. Compliance

5 new unit tests in `driver_adapter::tests`:

- CREATE / INSERT translate to the right
  `DriverResponse` variants.
- SELECT translates to `DriverResponse::Rows` with
  correct columns.
- Syntax error translates to `DriverError::Sql`.
- Adapter works behind `Box<dyn Driver>` (object
  safety verified).
- Clone shares engine state (creating a DB on one
  clone makes a duplicate-create on the other error
  correctly).

## 4. Honest status

**BRD-WIRE-001 "portable client driver API"** moves
from FOUNDATIONS to **PARTIAL**: one engine is wired.
Session engines, prepared statements, network
transport, streaming responses, and TLS remain OPEN
per §2.
