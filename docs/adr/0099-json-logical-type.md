# ADR 0099 — JSON logical type (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.**

## 1. Scope

- `LogicalType::Json` + `Value::Json(String)`, physical type
  `Utf8`.
- Canonical serialization via `serde_json` — parse validity
  checked at the application layer when constructing.
- Participates in the core `Ord` / `Hash` / `PartialEq`
  contract (tag 9, sorts after `Period`).

## 2. Out of scope

- JSON path queries (`->`, `->>`, `JSONPath`).
- Binary JSON (JSONB / BSON).
- Byte-identical whitespace / key-order preservation.
- Schema validation / JSON Schema.

## 3. Compliance

1. `Value::Json(canonical).logical_type() == Some(Json)`.
2. `LogicalType::Json.default_physical() == Utf8`.
3. Equal JSON strings compare equal; different ones not;
   `HashSet` works.
4. Tag-based ordering: Period < Json.
5. 4 tests in `tests/json_type.rs` pass.
