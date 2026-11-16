# ADR 0129 — Local driver facade trait (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch J J1 primitive.
Completes Batch J (J2 Prometheus rendering shipped in
ADR 0125).

## 1. Scope

New module `bradian_relational::driver`:

- `Driver` trait with a single object-safe method
  `execute(&self, sql: &str) -> DriverResult<DriverResponse>`.
- `DriverResponse` enum: `Empty` / `Affected { rows }` /
  `Rows(Vec<DriverRow>)`.
- `DriverRow { columns, values }` as the row envelope.
- `DriverError { NotWired / Sql / Resource }`.
- `NoopDriver` reference stub that records every
  submitted statement and returns `NotWired` — useful
  as a test double.

## 2. Out of scope (follow-ups)

- **Engine adapters** — wrapping the existing SQL
  engines (SqlEngine, MvccSqlEngine, DomainSqlEngine,
  HistorySqlEngine) behind this trait requires
  per-engine glue (schema pin, session mode, snapshot
  LSN, savepoint lifecycle). Each adapter ships in its
  own ADR.
- **Async / network driver** — tokio + wire protocol
  belong in a dedicated `bradian-wire` crate. BRD-SEC-003
  TLS/mTLS applies there.
- **Prepared statements** — one-shot execute only.
  Prepared-statement lifecycle needs a `PreparedStatement`
  handle type.
- **Transaction scope** — a `begin` / `commit` / `rollback`
  triplet is deferred until the session design is
  settled across all engines (ADR 0042 is per-engine).
- **Batch / multi-statement execution** — `execute`
  takes one statement; batch semantics need a dedicated
  method.
- **Typed column schema** — rows carry name-paired
  values; a richer `ColumnSchema { name, data_type }`
  envelope is a follow-up.

## 3. Compliance

- NoopDriver records every submitted statement.
- NoopDriver returns `NotWired` for every execute.
- `Driver` trait is object-safe (`Box<dyn Driver>`
  compiles).
- `NoopDriver` is `Send + Sync`.
- `DriverResponse` variants are distinct (Empty !=
  Affected != Rows).
- `log()` returns an immutable snapshot (not a live
  view).
- 6 new tests in `driver::tests`.

## 4. Honest status

Foundation only. **BRD-DRV-001 portable client driver
API** is not yet satisfied because no engine is wired
in. The trait is the contract future adapters and the
network driver will implement. Client code that depends
on `dyn Driver` today works against `NoopDriver` only.
