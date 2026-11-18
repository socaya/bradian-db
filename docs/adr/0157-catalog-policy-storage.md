# ADR 0157 — Catalog policy storage (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-18.** Promotes ADR 0123
`TableAccessPolicy` from a per-session parameter to a
first-class Catalog object persisted alongside the
table metadata. Caller no longer has to re-pass the
policy on every session — attach it once via the
Catalog API.

## 1. Scope

New field on `Table`:

- `policies: BTreeMap<String, TableAccessPolicy>` with
  `#[serde(default, skip_serializing_if = "is_empty")]`.
  Empty map preserves pre-ADR-0157 canonical schema
  bytes exactly (tested).

New methods on `Catalog`:

- `set_policy(id, name, policy)` — attach a named
  policy to a table. Overwrites any existing policy
  with the same name. `name` is validated as an
  identifier.
- `drop_policy(id, name)` — remove by name. No-op if
  absent.
- `get_policy(id, name) -> Option<&TableAccessPolicy>`.
- `policies_for_table(id) -> Option<&BTreeMap<String,
  TableAccessPolicy>>`.

All four `TableAccessPolicy` sub-types gained serde
derives in ADR 0156, so the storage round-trips through
JSON cleanly.

## 2. Out of scope (follow-ups)

- **`CREATE POLICY` DDL grammar** — the SQL parser
  still doesn't accept policy syntax. Callers attach
  policies programmatically via `Catalog::set_policy`.
  Grammar is a parser rewrite deferred to Gate 2.
- **Automatic catalog-driven engine wiring** — the
  engines still take an *explicit* `&TableAccessPolicy`
  on `with_session_policy` / `execute_with_policy`.
  A future ADR teaches the engines to look up policies
  from the catalog by table at SELECT time.
- **Policy mutation audit** — set/drop don't emit
  audit events yet.
- **Policy versioning / rollback** — catalog mutator
  does in-place overwrite; no history.
- **Cross-table / composite policies** — one policy
  attaches to one table.
- **Per-role / per-principal policies** — the current
  `TableAccessPolicy` is table-scoped; principal
  routing is caller-side via `AuthorizationContext`.

## 3. Compliance

New test file `catalog_policy_storage.rs` (9 tests):

- `set_policy_and_get_roundtrips`.
- `drop_policy_removes_it`.
- `set_policy_overwrites_existing_name`.
- `policies_for_table_lists_every_attached_policy`.
- `set_policy_unknown_table_errors`.
- `drop_policy_on_unknown_name_is_no_op`.
- `table_without_policies_has_empty_map`.
- `policy_round_trips_through_serde_json`.
- `legacy_table_without_policies_field_still_deserializes`
  — critical backward-compat test.

## 4. Honest status

**BRD-SEC-002 "row/column/tenant policies end-to-end"**
coverage: RLS + CLS + tenant on SELECT (all four
engines) + catalog storage + serde round-trip. The
remaining OPEN work is the `CREATE POLICY` grammar
(parser rewrite, Gate 2) and automatic
catalog→engine wiring at SELECT time.
