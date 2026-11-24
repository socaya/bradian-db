# ADR 0257 — Scalar CAST extends to TIMESTAMPTZ and UUID

## Status

Accepted.

## Context

CAST was already in the grammar for `BOOLEAN / INT / BIGINT /
TEXT / JSON` (ADR 0235). Two important types were absent:
`TIMESTAMPTZ` and `UUID`. That meant users of those columns
could not convert them for projection or extraction. P0-3
conformance requires coverage of the declared types a table
can carry.

## Decision

- `parse_cast_target` accepts `timestamp` / `timestamptz` and
  `uuid` as new target keywords.
- `cast_value` adds:
  - `BIGINT|INT -> TIMESTAMPTZ` (treats the integer as unix
    microseconds).
  - `TIMESTAMPTZ -> BIGINT` (micros back out).
  - `TIMESTAMPTZ -> TEXT|VARCHAR` (micros as decimal string
    — a canonical timezone-aware format is a later slice).
  - `TEXT -> UUID` via a new `parse_uuid_text` helper that
    accepts the canonical 36-char hyphenated form (case
    insensitive on hex).
  - `UUID -> TEXT|VARCHAR` via `format_uuid_text` (canonical
    lowercase hyphenated).
- NULL propagates (already handled at the top of
  `cast_value`).

## Consequences

- Scalar CAST on columns in SELECT projection now works for
  the full declared type matrix the base engine supports,
  modulo `Decimal(p,s)` which needs precision/scale
  parsing — that is a separate ADR.
- The TIMESTAMPTZ text form is currently a decimal micros
  string, not a `YYYY-MM-DD HH:MM:SS.ffffff+00` format. A
  future slice can add proper formatting once calendar
  arithmetic is admitted.
- 12 cases in `tests/scalar_cast.rs` cover: int→bigint,
  text→bigint, bigint→text, bad text→bigint rejected,
  bigint↔timestamptz round-trip, timestamptz→text,
  uuid round-trip, invalid-uuid-text rejected,
  bool-through-int, unsupported target rejected, NULL
  propagates, scalar WHERE still works unaltered.
