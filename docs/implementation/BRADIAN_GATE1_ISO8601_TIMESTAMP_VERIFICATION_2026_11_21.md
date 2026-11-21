# Master Gate 1 — ISO 8601 TIMESTAMPTZ literal verification

21 November 2026. ADR 0180. Parser extension closing the
ADR 0113 §2 "ISO-string timestamp literals" follow-up.

## Executed result

| Executed check | Result |
|---|---|
| `cargo test --locked -p bradian-relational --test iso8601_timestamp` | 7 passed, 0 failed |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## Finding and action

ADR 0113 accepted TIMESTAMPTZ columns in CREATE TABLE
but `coerce` accepted only integer-micros literals.
Operators who held wall-clock timestamps in standard
ISO 8601 had to pre-convert to micros before INSERT.

Change (ADR 0180):

- `coerce` for `(DataType::TimestampTz, Literal::Text(s))`
  parses `YYYY-MM-DDTHH:MM:SS[.ffffff][Z|±HH:MM]` via a
  new `parse_iso8601_micros` helper.
- Howard Hinnant's days-from-civil algorithm for the
  date part; no external dependency.
- Exact microsecond precision; seven or more
  fractional digits rejected.
- Offsets subtracted to normalize to UTC-stored micros.
- Leap-second minute (`:60`) accepted without
  adjustment (documented honest boundary).
- Any deviation returns `SqlError::Type` with the
  source literal.

Added tests
(`crates/bradian-relational/tests/iso8601_timestamp.rs`,
7 total):

1. Zulu epoch round-trips to 0 micros.
2. Microsecond-precision round-trip.
3. Positive-offset UTC shift.
4. Negative-offset UTC shift.
5. Integer-micros path still works.
6. Seven malformed-literal rejection cases.
7. Leap-second minute accepted.

## Scope limits (honest)

- Named time zones not supported — numeric offsets
  only.
- Partial ISO 8601 forms not supported (date-only,
  week date, ordinal date).
- Sub-microsecond precision rejected (seven+ fractional
  digits).
- DATE / TIME / INTERVAL types not yet reachable
  through SQL.
- CURRENT_TIMESTAMP / NOW() expression grammar not
  parsed; `DefaultExpr::CurrentTimestamp` remains the
  only clock-sourced primitive.

## Evidence

- Source:
  `crates/bradian-relational/src/sql.rs` — new
  `parse_iso8601_micros` helper; `coerce`
  TimestampTz-Text branch.
- Tests:
  `crates/bradian-relational/tests/iso8601_timestamp.rs`.
- ADR: `docs/adr/0180-iso8601-timestamp-literals.md`.
