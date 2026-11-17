# ADR 0155 — Column-level projection wiring (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-17.** Activates the CLS
(column-level security) dimension of ADR 0123's
`TableAccessPolicy` across all four SQL engines.

## 1. Scope

Three narrow additions:

1. **`ColumnLevelPolicy::is_configured()`** — returns
   `!self.readable.is_empty()`. For engine integration
   an empty allow-list means "CLS not configured"
   (caller passed a placeholder). Non-empty means
   "explicit allow-list".
2. **`PolicyError::ColumnDenied { column }`** — new
   error variant raised when a caller's explicit
   SELECT asks for a denied column.
3. **SELECT-path CLS wiring** in all four engines
   (`SqlEngine`, `HistorySqlEngine`, `DomainSqlEngine`,
   `MvccSqlEngine`): after parsing the selected column
   list, if the policy's CLS is configured:
   - `SELECT *` → silently retain only readable columns.
   - `SELECT col1, col2, …` → error with `ColumnDenied`
     on any denied column.

## 2. Out of scope (follow-ups)

- **Mutation-side CLS** — write statements don't
  consult CLS (per ADR 0123 scope).
- **Row-shape re-projection downstream** — the output
  columns reflect the filter, but the engines'
  internal `indices` are rebuilt from the filtered
  `columns` list so no stale indices leak.
- **Policy+budget composition** — ADR 0156
  (companion).
- **Catalog storage** — policies passed per-session;
  ADR 0157 (companion) adds catalog storage.

## 3. Compliance

- `SELECT *` with `readable = {id, name}` returns
  columns `[id, name]` (tenant omitted).
- `SELECT tenant` with the same CLS errors with
  `PolicyError::ColumnDenied { column: "tenant" }`.
- `deny_all()` empty readable set is treated as "CLS
  disabled" — preserves the ADR 0153/0154 placeholder
  pattern.
- 3 new tests in `sql_engine_policy.rs` (total 10).

## 4. Honest status

**BRD-SEC-002** now covers RLS + CLS + tenant on SELECT
across all four engines. Write-side authz and catalog
storage remain OPEN (ADR 0156 + 0157 follow).
