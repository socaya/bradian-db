# ADR 0180 — ISO 8601 `TIMESTAMPTZ` text literals (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-21.** Closes the ADR 0113 §2
follow-up:

> **ISO-string timestamp literals** — current accepts only
> integer micros.

## 1. Scope

`SqlEngine::coerce` for `(DataType::TimestampTz,
Literal::Text(s))` parses ISO 8601 strings of the form
`YYYY-MM-DDTHH:MM:SS[.ffffff][Z|±HH:MM]` into
microseconds since the Unix epoch and emits
`Value::TimestampTz(micros)`.

Accepted forms:

- `Z` designator — explicit UTC.
- `±HH:MM` offset — offset is subtracted to normalize
  to UTC-stored micros.
- Optional fractional seconds up to six decimal digits
  (microsecond precision).
- ` ` (space) in place of `T` as the date/time
  separator — a common ISO-adjacent spelling.
- Leap-second minute (`:60`) — recorded without
  adjustment; operators who need strict POSIX may
  clamp upstream.

The integer-micros path (ADR 0113) remains unchanged:
`INSERT ... VALUES(1731700000000000)` still works.

Implementation: a single `parse_iso8601_micros(&str) ->
Option<i64>` helper using Howard Hinnant's
days-from-civil algorithm. No external dependency; no
runtime allocation; exact microsecond precision; any
unknown form returns `SqlError::Type`.

## 2. Out of scope

- **Named time zones** (`America/New_York`). Only
  numeric offsets.
- **Partial ISO 8601 forms** (date-only, week date
  `2026-W47-6`, ordinal date `2026-325`). Only the
  full `YYYY-MM-DDTHH:MM:SS` form.
- **Sub-microsecond precision.** Seven or more
  fractional digits are rejected.
- **DATE / TIME / INTERVAL types.** Not yet reachable
  through SQL.
- **CURRENT_TIMESTAMP / NOW()** expressions.
  `DefaultExpr::CurrentTimestamp` ships separately and
  takes its clock from the caller.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/iso8601_timestamp.rs`
(7 tests, all green):

1. `iso_8601_zulu_epoch_round_trips_as_zero_micros` —
   `'1970-01-01T00:00:00Z'` → 0 micros.
2. `iso_8601_zulu_with_microseconds_preserves_exact_precision`
   — `.123456` survives round-trip with
   `micros % 1_000_000 == 123_456`.
3. `iso_8601_positive_offset_shifts_back_to_utc` — two
   literals with equivalent UTC time stored equal.
4. `iso_8601_negative_offset_shifts_forward_to_utc`.
5. `iso_8601_integer_micros_still_accepted` — legacy
   path.
6. `iso_8601_rejects_malformed_literals` — seven
   negative cases: free text, wrong separators, month
   13, hour 25, offset 99, seven fractional digits.
7. `iso_8601_leap_second_minute_is_accepted`.

Clippy clean on `-D warnings`.

## 4. Roadmap impact

BRD-SQL-006 TIMESTAMPTZ literal reach is now text or
integer; the last ADR 0113 §2 numeric-only limitation
is closed. The remaining BRD-SQL-006 work (collation
beyond ASCII, full documented isolation qualification,
expression-level date arithmetic) is unaffected.
