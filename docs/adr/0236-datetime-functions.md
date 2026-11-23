# ADR 0236 — Date/time functions

## Status

Accepted.

## Context

Timestamp columns are stored as microseconds since the Unix
epoch (`Value::TimestampTz(i64)`). SELECT lacked any built-in
way to produce the current timestamp or shift a stored value.

## Decision

Three `FuncExpr` variants:

- `Now` — calls `SystemTime::now()` per row, returns
  `Value::TimestampTz(micros)`.
- `TimestampAdd { ts, seconds }` — adds a signed integer
  seconds offset. Overflow rejects with `SqlError::Type`.
- `TimestampDiff { a, b }` — `(a - b)` returned as whole
  seconds (`Value::Int64`).

NULL propagates through all three.

## Consequences

- Deliberately minimal. No `DATE_TRUNC`, no `EXTRACT`, no
  timezone or calendar-aware arithmetic. These need a datetime
  library; future ADR when the scope demands it.
- `Now` is evaluated per-row rather than per-statement; in
  practice both are indistinguishable at microsecond
  resolution for small result sets.
