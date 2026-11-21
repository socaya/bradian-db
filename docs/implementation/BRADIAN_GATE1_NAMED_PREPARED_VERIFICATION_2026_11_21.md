# Master Gate 1 — Named prepared-parameter verification

21 November 2026. ADR 0183. Closes the ADR 0139 §2 /
ADR 0113 §2 named-parameter follow-up.

## Executed result

| Executed check | Result |
|---|---|
| `cargo test --locked -p bradian-relational --test named_prepared` | 10 passed, 0 failed |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## Finding and action

`PreparedStatement` supported only positional `$N`
placeholders. The envelope could not express the
keyword-argument style (`:name`) that most database
clients prefer, and `Driver::execute_prepared` had no
path to bind named values.

Change (ADR 0183):

- `PreparedStatement.named_params: BTreeMap<String,
  Value>` with `#[serde(default,
  skip_serializing_if = empty)]`. Legacy envelopes
  decode and re-encode identically.
- `validate()` extended with per-name checks and the
  three new error variants
  (`UnknownNamedParameter`,
  `UnusedNamedParameter`,
  `InvalidNamedParameter`).
- New `scan_named_placeholders(sql)` scanner.
- New `bind_named_literals(template, named_params)`
  binder that runs after `bind_literals`, enabling
  mixed positional + named templates.
- `Driver::execute_prepared` default impl binds
  positional first, then named.
- `PreparedStatement::with_named(sql, named_params)`
  convenience constructor.

Added tests
(`crates/bradian-relational/tests/named_prepared.rs`,
10 total): named-only bind; mixed positional + named;
missing/unused binding error paths; invalid identifier
rejection; repeated references; scan set semantics;
colon-without-identifier handling; serde round-trip
plus backward-compat decode; `execute_prepared` log
confirmation on `NoopDriver`.

## Scope limits (honest)

- Scan is text-level, not lexer-aware; `:name` inside
  a `'...'` string literal counts (same honest
  limitation already documented on the positional
  scanner).
- Only `:name` sigil; `@name` deferred.
- No server-side plan caching; textual rebinding each
  call.
- No type pre-validation against the catalog.

## Evidence

- Source:
  `crates/bradian-relational/src/prepared.rs` — new
  field, validation, scanner, new error variants.
  `crates/bradian-relational/src/driver.rs` — new
  `bind_named_literals`; `execute_prepared` wires
  both binders.
- Tests:
  `crates/bradian-relational/tests/named_prepared.rs`.
- ADR: `docs/adr/0183-named-prepared-parameters.md`.
