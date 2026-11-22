# ADR 0203 — JSON array CONCAT on UPDATE

## Status

Accepted.

## Context

ADR 0201 added single-element `INSERT` for JSON arrays. Callers also
need to append a batch of elements in one statement. The required
grammar is `UPDATE … SET <col> <path> CONCAT '<json-array-literal>'`
where the terminal of the path must be an array in the stored row.

## Decision

Introduce a `CONCAT` terminal verb on JSON path SET elements across
the three online engines (`SqlEngine`, `DomainSqlEngine`,
`MvccSqlEngine`) and the historical `HistorySqlEngine`.

- Parser (`path_assignments`, `mixed_assignments`): after a JSON
  path LHS, admit `CONCAT <literal>` symmetric with `REMOVE` /
  `INSERT` / `=`.
- `SetAssignment::JsonConcat { column, path, value }` and
  `HistoryAssignment::JsonConcat { … }` carry the parsed form.
- `apply_json_path_concat` + `descend_and_concat`:
  - Reparse the literal as JSON; it must decode to a JSON array
    (`SqlError::Type` otherwise).
  - Walk the path requiring intermediate objects/arrays to exist
    (no auto-creation — unlike `apply_json_path_mutation` — because
    CONCAT targets an existing array).
  - At the terminal require an array; extend it with every element
    of the RHS.
  - An empty path means the column cell is itself the target array.
- `Slice` segments anywhere on the path are rejected
  (`SqlError::Unsupported`).
- UPDATE execution order: column assignments → JSON path `=` → JSON
  removes → JSON inserts → JSON concats. CHECK, FK, and UNIQUE run
  against the final mutated row.

## Consequences

- One statement can prepend / splice / append via INSERT + CONCAT.
- No support for element-at-a-time `||` (SQL-standard spelling). We
  spell this as a dedicated verb to keep the parser single-pass.
- Validated by `tests/json_path_concat.rs` (13 cases covering empty
  RHS, mixed types, container form, non-array RHS/target rejection,
  bad JSON, interaction with INSERT, slice rejection, and both
  DomainSqlEngine and MvccSqlEngine) plus the historical
  `history_concat_appends_array_literal` case.
