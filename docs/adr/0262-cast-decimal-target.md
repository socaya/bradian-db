# ADR 0262 — DECIMAL(p,s) CAST target

## Status

Accepted.

## Context

CAST targets covered INT / BIGINT / TEXT / BOOLEAN / JSON /
TIMESTAMPTZ / UUID (ADRs 0235, 0257) but not DECIMAL(p,s).
With DECIMAL as a storable type, CAST on arithmetic results
and text literals could not land back in a DECIMAL column.

## Decision

- `parse_cast_target` now delegates to `Parser::data_type`,
  so CAST accepts every target the CREATE TABLE grammar
  does, including `DECIMAL(p, s)`, `VARCHAR(n)`.
- `cast_value` adds four new branches for DECIMAL target:
  - `Decimal → Decimal` (same-scale re-check; fit against new
    precision).
  - `Int32 → Decimal` and `Int64 → Decimal` (scale up by
    `scale` zeros).
  - `String → Decimal` via new `parse_decimal_text` helper
    that accepts optional sign, integer part, and up to
    `scale` fraction digits (longer fractions reject).
- New `decimal_fit(value, precision, scale)` helper enforces
  `|value| < 10^precision`.

## Consequences

- Fraction digits BEYOND the target scale are rejected (no
  rounding or truncation — the user must make the decision
  explicit).
- Overflow against precision rejects with `SqlError::Type`.
- `parse_decimal_text` is ASCII-only, no thousands separators,
  no scientific notation. Future slices can broaden the
  accepted formats.
- 8 cases in `tests/cast_decimal.rs` cover: int→decimal
  with scale zeros; text decimal→decimal; negative;
  too-many-fractions reject; overflow reject; int-too-wide
  reject; zero; decimal→decimal chain.
