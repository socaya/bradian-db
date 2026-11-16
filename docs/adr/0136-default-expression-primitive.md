# ADR 0136 — DEFAULT expression primitive (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch B B1 primitive —
completes Batch B. Extends ADR 0124 `Catalog::set_column_
default` beyond literal values.

## 1. Scope

New module `bradian_relational::default_expr`:

- `DefaultExpr` enum (serde-serialized for a future
  catalog journal):
  - `Literal(Value)` — unchanged behavior from ADR 0124.
  - `CurrentTimestamp` — evaluated to the caller's
    clock reading at INSERT time.
  - `NullExpr` — explicit `Value::Null`; only valid on
    nullable columns.
- `DefaultExpr::validate(&Column) -> Result<()>` — pure
  type/nullability check.
- `DefaultExpr::eval(now_micros: i64) -> Value` — no
  internal clock; caller injects micros since UNIX
  epoch so tests are deterministic.
- `DefaultExprError { LiteralTypeMismatch /
  CurrentTimestampOnNonTimestamp / NullDefaultOnNotNull }`.

## 2. Out of scope (follow-ups)

- **Catalog storage** — `column_defaults: BTreeMap<String,
  Value>` still stores literal `Value`s. A follow-up ADR
  migrates to `BTreeMap<String, DefaultExpr>` with
  serde-driven JSON round-trip, including version-2
  catalog recovery (preserving empty-map legacy bytes
  per ADR 0040).
- **SQL parser `DEFAULT CURRENT_TIMESTAMP` / `DEFAULT
  NULL`** — parser still accepts literal defaults only.
- **Richer expressions**: arithmetic (`a + 1`),
  function calls (`uuid_generate_v4()`), column refs,
  subqueries, sequence refs. Each is a dedicated ADR.
- **Deterministic replay** — `CURRENT_TIMESTAMP` is
  evaluated inside INSERT execution; WAL must record
  the materialized value, not the expression, so
  recovery sees the same clock reading.

## 3. Compliance

- `Literal(v)` validates/evals identically to ADR 0124.
- Literal type mismatch rejected.
- Literal NULL on NOT NULL column rejected; accepted on
  nullable.
- `CurrentTimestamp` on TimestampTz validates; evaluates
  to the caller-supplied micros.
- `CurrentTimestamp` on non-TimestampTz rejected.
- `NullExpr` only valid on nullable columns; evaluates
  to `Value::Null`.
- Varchar length bound + JSON content validated for
  literals.
- serde JSON round-trip preserves every variant.
- 11 new tests in `default_expr::tests`.

## 4. Honest status

Foundation only. **BRD-DDL-006 DEFAULT expressions** is
not yet satisfied because catalog storage still holds
literals, the SQL parser doesn't accept expression
defaults, and WAL replay of `CURRENT_TIMESTAMP` needs
the materialized-value record. The primitive is the
prerequisite that a future catalog + parser ADR
consumes.
