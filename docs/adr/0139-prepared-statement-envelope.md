# ADR 0139 — Prepared-statement envelope (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Companion to ADR 0129
(`Driver` trait) and ADR 0137 (wire-frame envelope). The
envelope travels inside a Request frame's payload.

## 1. Scope

New module `bradian_relational::prepared`:

- `PreparedStatement { sql: String, params: Vec<Value> }`
  — serde-serialized for wire transport.
- `PreparedStatement::validate()` — syntactic check:
  - Rejects empty SQL.
  - Rejects `$0` (positional placeholders are 1-indexed).
  - Rejects `$N` with `N > params.len()`.
  - Rejects unused parameters (`used < supplied`).
- `scan_placeholders(sql)` collects the distinct `$N`
  positional references. Repeated references to the same
  `$N` count once (both reads bind to the same
  parameter).
- `PreparedError { EmptySql / ZeroIndexedPlaceholder /
  PlaceholderOutOfRange / UnusedParameters }`.

## 2. Out of scope (follow-ups)

- **Engine binding** — the Driver trait doesn't accept
  prepared statements; binding is per-engine ADR work.
- **Named parameters** (`:name`, `@name`) — positional
  only.
- **Type pre-validation against catalog schema** — the
  envelope checks count + placeholder range only. Binding
  `params[i]` against a column's `DataType` happens at
  execution time.
- **Plan caching / re-use** — a future ADR introduces a
  prepared-statement cache handle.
- **Lexer-aware placeholder scan** — the current textual
  `$digits` scan would count `$1` inside a string
  literal. Follow-up SQL-lexer work can refine.

## 3. Compliance

- Simple one-param statement validates.
- Repeated `$1` with one supplied param is OK.
- Multiple placeholders in any order validate.
- `$5` with 2 params → `PlaceholderOutOfRange`.
- One placeholder + 2 supplied → `UnusedParameters`.
- `$0` → `ZeroIndexedPlaceholder`.
- Empty SQL → `EmptySql`.
- No placeholders + no params → OK.
- `$1abc` scans as `$1` + `abc` (greedy digit match).
- `$foo` with no digits is literal (ignored).
- serde JSON round-trip preserves envelope exactly.
- 11 new tests in `prepared::tests`.

## 4. Honest status

Foundation only. **BRD-DRV-002 prepared statements** is
not yet satisfied because no engine accepts the
envelope, type pre-validation isn't performed, and no
lexer-aware scan guards against placeholders inside
string literals. The envelope is the prerequisite that
future driver + engine work consumes.
