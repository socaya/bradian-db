# ADR 0259 — Expression DEFAULT

## Status

Accepted.

## Context

Literal defaults (ADR 0035) only cover constant values. Users
need `DEFAULT CURRENT_TIMESTAMP` on audit-style columns. Also,
INSERT required every column to be listed, so even nullable
columns had to be spelled out — a real usability drag.

## Decision

- New `model::DefaultExpr::CurrentTimestamp` enum, serde
  SCREAMING_SNAKE_CASE. Deliberately a tiny closed set so the
  catalog format stays portable.
- `Table.column_default_exprs: BTreeMap<String, DefaultExpr>`
  with serde default + skip-if-empty for format compat.
- `Catalog::set_column_default_expr` / `drop_column_default_expr`.
  SET bumps schema_version; validates TIMESTAMPTZ target.
  Setting an expression default removes any literal default
  for the same column (resolution unambiguous).
- `PersistentCatalog::Command::SetColumnDefaultExpr` /
  `DropColumnDefaultExpr`; public methods `set_column_default_expr`
  / `drop_column_default_expr`.
- SQL: `ALTER TABLE t ALTER COLUMN c SET DEFAULT CURRENT_TIMESTAMP`
  and `… SET DEFAULT NOW()` dispatch to the expression path.
  `DROP DEFAULT` drops both kinds.
- INSERT VALUES omitted-column path: expression default →
  literal default → NULL (if nullable) → error. The old
  "must list all columns" check is replaced with a per-column
  check that still fails for NOT NULL columns without a default.
- `evaluate_default_expr(CurrentTimestamp)` reads the system
  clock and returns `Value::TimestampTz(micros)`. Clock
  failures return epoch-0 (never block an INSERT; operator-
  visible via the resulting value).

## Consequences

- Only `CURRENT_TIMESTAMP` / `NOW()` are expression defaults.
  Other builtins (`gen_random_uuid()`, `uuid_generate_v4()`)
  are deferred to future slices.
- Expression evaluation is non-deterministic across rows
  (clock advances) — the two rows in a single INSERT VALUES
  batch can receive slightly different timestamps. A future
  slice could snapshot the statement start once.
- INSERT grammar now accepts omitted columns. Existing tests
  that used the "all columns required" error still work because
  they all list every column.
- 9 cases in `tests/expression_default.rs` cover: fills
  omitted column; NOW() equivalent; NOW without parens
  rejected; CURRENT_TIMESTAMP on non-timestamp rejected;
  DROP DEFAULT removes it; omit NOT NULL without default
  rejected; literal default still works on omit; expression
  overrides literal; persists across reopen.
