# ADR 0212 — ORDER BY / LIMIT / OFFSET on the base engines

## Status

Accepted.

## Context

G44 (ADR 0039) added typed `ORDER BY ... [ASC|DESC] [NULLS FIRST|LAST]`
with `LIMIT` and `OFFSET` to `HistorySqlEngine`, implemented as a
reusable `select_window::SelectWindow` module. The three online
engines (`SqlEngine`, `DomainSqlEngine`, `MvccSqlEngine`) still
rejected the clause.

## Decision

Reuse `SelectWindow` in all three online engines.

- `SelectWindow::parse(&mut parser, &table)` runs after
  `parse_select_where` in each engine's SELECT branch; the
  resulting window is applied after the post-scan JSON WHERE
  filter and before projection.
- `window.input_row_limit()` caps the scan to 10 000 rows via
  `ScanControl::cap_rows`, matching the historical engine's bound.
- `window.apply(rows, &table)` performs the stable sort (16-key
  max, logical primary-key tie-break, per-column `COLLATE` from
  ADR 0175) and the `OFFSET` / `LIMIT` slice.
- Semantics preserved verbatim from G44: `ASC` default; `NULLS
  LAST` for ASC and `NULLS FIRST` for DESC unless explicit;
  `LIMIT ≤ 10 000`, `OFFSET ≤ 1 000 000`; `LIMIT` grammatically
  precedes `OFFSET` (reverse order unsupported, same as G44).

## Consequences

- No module changes outside the three engines; `SelectWindow`
  stays in `select_window.rs`.
- Admission caps (rows / bytes) still apply BEFORE the window is
  applied; the cap keeps a windowed query from materializing
  more than 10 000 rows regardless of the eventual LIMIT.
- No ORDER BY on an expression, no window functions, no
  cross-engine streaming sort — explicitly out of scope.
- Validated by `tests/base_order_by.rs` (15 cases: text / numeric
  ASC / DESC, PK tie-break, two-key composite, LIMIT / OFFSET
  interaction, NULLS LAST default, error cases for unknown
  column and cap exceed, PK WHERE composition, Domain + MVCC
  parity). The historical G44 tests continue to pass with the
  shared module.
