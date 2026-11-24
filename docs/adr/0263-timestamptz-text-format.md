# ADR 0263 — TIMESTAMPTZ text format (ISO-8601 round-trip)

## Status

Accepted.

## Context

ADR 0257 rendered `CAST(ts AS TEXT)` as a decimal-micros
integer string. That form is not what any SQL client
expects. ADR 0180 already parses ISO-8601 text into
TIMESTAMPTZ; the render should be its inverse so round-trip
is stable.

## Decision

- New `format_timestamptz_text(micros)` renders
  `YYYY-MM-DDTHH:MM:SS.ffffff+00:00`. Everything in UTC.
  Fixed-width fields; subsecond precision is 6 digits.
- New `civil_from_days(z)` is Howard Hinnant's
  civil-from-days algorithm, the exact inverse of ADR 0180's
  days-from-civil parser. Handles negative days (pre-epoch).
- `cast_value` for `TIMESTAMPTZ -> TEXT|VARCHAR` dispatches
  through `format_timestamptz_text` instead of
  `n.to_string()`.
- `cast_value` for `TEXT -> TIMESTAMPTZ` now delegates to
  `parse_iso8601_micros` (ADR 0180) so the round-trip is
  symmetric.
- Old canary test (`cast_timestamptz_to_text` in
  `tests/scalar_cast.rs`) updated to the new format.

## Consequences

- Clients get the ISO-8601 form expected by SQL and JSON
  tooling. Pre-ADR-0263 callers that compared against the
  decimal-micros string will break — the two canary
  assertions have been updated; one application-side flip
  is needed per ADR 0180 ↔ 0263 pairing.
- No timezone support yet. The string ends with `+00:00`
  always. A later slice can render local offsets once a
  session timezone exists.
- Pre-epoch timestamps (negative micros) render correctly
  via `rem_euclid` / `div_euclid` on the micros.
- 5 cases in `tests/cast_timestamp_text.rs` cover: epoch,
  recent, pre-epoch, round-trip, fractional micros.
