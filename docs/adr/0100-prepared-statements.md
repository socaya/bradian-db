# ADR 0100 — Prepared statements (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.**

## 1. Honest scope

`SqlEngine::prepare(template)` returns a `PreparedStatement`
handle that caches a SQL template with `?` placeholders.
Execution substitutes properly-escaped `Value` literals and
runs through `execute()` / `execute_as()`.

This is a **text-template facility**. It is NOT a parsed-AST
plan cache; the SQL is re-parsed on each execute. Future work
can upgrade the internals without a public API change.

## 2. Out of scope

- Parsed-AST plan caching.
- Named parameters (`:name`).
- Type checking of bound parameters against the target
  column's logical type.
- Parameter arrays / batch binds.

## 3. Compliance

1. `prepare` counts `?` placeholders; `?` inside `'…'`
   literals are not counted.
2. Unterminated string literals rejected with `Syntax` error.
3. `execute(engine, params)` runs the materialised SQL
   through the engine.
4. `execute_as(engine, authz, params)` honours ADR 0098.
5. Parameter count mismatch returns `Unsupported`.
6. String params are SQL-escaped (single quotes doubled).
7. 6 tests in `tests/prepared_statements.rs`.
