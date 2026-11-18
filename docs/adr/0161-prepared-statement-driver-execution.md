# ADR 0161 — Prepared-statement driver execution (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-18.** Hooks ADR 0139's
`PreparedStatement` envelope into the `Driver` trait so
every driver gets prepared-statement execution for free.

## 1. Scope

- `Driver::execute_prepared(&self, &PreparedStatement) ->
  DriverResult<DriverResponse>` with a default
  implementation that:
  1. Validates the envelope (`stmt.validate()`).
  2. Binds parameters via `bind_literals(sql, params)`.
  3. Delegates to `self.execute(&bound_sql)`.
- `bind_literals(template, params)` — public helper
  that walks the SQL replacing `$N` placeholders with
  literal SQL. Each `Value` variant gets a sensible
  rendering:
  - `Null` → `NULL`, `Boolean` → `TRUE`/`FALSE`,
    `Int32`/`Int64`/`Decimal`/`TimestampTz` → numeric,
    `Uuid` → `'hex32'`, `String` → single-quote-escaped.
- Rejects `$0` and out-of-range placeholders with
  `DriverError::Sql`.

## 2. Out of scope

- **Server-side prepared plans / plan cache** —
  textual rebinding on every call for now. Native
  drivers can override `execute_prepared` for caching.
- **SQL injection hardening** — single-quote escape
  only; the surface is a privileged in-process trust
  boundary. Future `bradian-serving` network driver
  will need stronger validation at the transport edge.
- **Named parameters** (`:name`, `@name`) —
  positional only (ADR 0139 scope).

## 3. Compliance

6 new unit tests in `driver::tests`:

- positional substitution round-trips.
- single-quote escape in strings.
- out-of-range placeholder rejected.
- zero-indexed placeholder rejected.
- Null / Boolean / Uuid render correctly.
- `NoopDriver::execute_prepared` records bound SQL.

## 4. Honest status

Every `Driver` implementation (base + 3 session
adapters + future network drivers) now supports
prepared statements with zero per-impl boilerplate.
