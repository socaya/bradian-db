# ADR 0183 — Named parameters in prepared statements (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-21.** Closes the ADR 0139 §2
and ADR 0113 §2 follow-up:

> **Named parameters** (`:name`, `@name`) — positional
> only (ADR 0139 scope).

## 1. Scope

- `PreparedStatement` gains a
  `named_params: BTreeMap<String, Value>` field.
  `#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]`
  so pre-ADR-0183 wire envelopes decode and re-encode
  identically.
- Validation extended:
  - Every `:name` reference must have a binding
    (`PreparedError::UnknownNamedParameter`).
  - Every binding must be referenced
    (`PreparedError::UnusedNamedParameter`).
  - Parameter name must match
    `^[A-Za-z_][A-Za-z0-9_]*`
    (`PreparedError::InvalidNamedParameter`).
- `scan_named_placeholders(sql) -> BTreeSet<String>` —
  text-level scan, same honest limitation as the
  positional `scan_placeholders` (no literal-string
  awareness yet).
- `bind_named_literals(template, &BTreeMap<_, _>)` in
  `driver` runs after `bind_literals`; one template can
  mix `$N` and `:name` safely.
- `Driver::execute_prepared` binds positional first,
  then named. Validation is done once up-front via
  `stmt.validate()`.
- `PreparedStatement::with_named(sql, named_params)`
  convenience constructor for named-only envelopes.

## 2. Out of scope

- **Lexer-aware scan.** Still inherits the
  "`:name` inside a `'...'` string literal gets counted"
  limitation documented on the positional scan. A future
  slice can promote both scans to lex-level.
- **`@name` sigil.** `:name` only. `@name` can be added
  by a small lexer tweak when there's a concrete use.
- **Server-side plan caching.** Still textual rebinding
  per call.
- **Type pre-validation against catalog.** Binding by
  name does not yet consult the target column's
  `DataType`.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/named_prepared.rs`
(10 tests, all green):

1. `named_only_statement_validates_and_binds` — pure
   `:name` template binds correctly.
2. `positional_and_named_may_coexist_in_one_template`
   — mixed `$1` + `:name` bind through the full
   `execute_prepared` path.
3. `missing_named_binding_fails_validate` —
   `UnknownNamedParameter { name }`.
4. `unreferenced_named_binding_fails_validate` —
   `UnusedNamedParameter { name }`.
5. `invalid_named_identifier_is_rejected_at_validate`
   — empty name and `"1foo"` both rejected.
6. `repeated_named_reference_binds_the_same_value` —
   `:id, :id, :id` with one binding substitutes
   identically.
7. `scan_named_placeholders_returns_distinct_names` —
   set semantics, sorted output.
8. `colon_without_identifier_is_literal` — `:5` and
   `::foo` interpreted per the scan rule.
9. `serde_round_trip_preserves_named_params` — and
   backward-compat: legacy JSON without
   `named_params` decodes with an empty map.
10. `execute_prepared_binds_both_styles_through_driver_trait`
    — `NoopDriver` records the final bound SQL in its
    log, confirming the `execute_prepared` default
    impl invokes both binders in order.

Clippy clean on `-D warnings`.

## 4. Roadmap impact

- ADR 0139 §2 "Named parameters" closed.
- ADR 0113 §2 remaining follow-up items after this ADR:
  - JSON path operators (deferred with ADR 0173 §2).
  - FOREIGN KEY action keywords (parser-only; actual
    FK enforcement is ADR 0101 §2's deferred separate
    ADR).
