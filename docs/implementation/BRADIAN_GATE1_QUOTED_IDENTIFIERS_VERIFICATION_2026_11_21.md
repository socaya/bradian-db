# Master Gate 1 — Quoted identifier verification

21 November 2026. ADR 0181. Closes the ADR 0112/0113 §2
"Quoted identifiers" follow-up.

## Executed result

| Executed check | Result |
|---|---|
| `cargo test --locked -p bradian-relational --test quoted_identifiers` | 7 passed, 0 failed |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## Finding and action

The SQL lexer previously rejected the double-quote
character outright. Operators who needed a column
named with a reserved word (e.g. `select`, `primary`)
had no way to express it; they had to rename the
column.

Change (ADR 0181):

- New `Token::QuotedWord(String)` variant in the
  lexer. `"..."` is parsed; `""` inside is an escaped
  double-quote.
- `Parser::word()` accepts both `Token::Word` and
  `Token::QuotedWord`. Both lowercase at the boundary
  to preserve ADR 0006 case-insensitive catalog
  semantics.
- Keyword matching (`consume_word`, `next_is_word`)
  only matches `Token::Word`, so `"PRIMARY"` is an
  identifier, not the `PRIMARY KEY` clause header.
- Empty and unterminated quoted identifiers surface
  as `SqlError::Syntax`. Catalog's ASCII-identifier
  rule still applies after the lexer, so content
  like `has"quote` passes the lexer and fails at the
  catalog.

Added tests
(`crates/bradian-relational/tests/quoted_identifiers.rs`,
7 total): CREATE TABLE + INSERT + SELECT with quoted
3-part names; reserved-word-as-column round-trip;
quoted/unquoted equivalence through the catalog;
escaped-quote lex-only acceptance; empty rejection;
unterminated rejection; keyword-doesn't-match on
quoted form.

## Scope limits (honest)

- **Case not preserved.** The parser lowercases at the
  identifier boundary to match the catalog's ADR 0006
  storage rule. Quoting lets reserved words name
  user objects; it does not introduce case-sensitive
  identifiers.
- **ASCII only.** Catalog's `identifier()` still
  requires ASCII alphanumeric + underscore; quoted
  non-ASCII content lexes but fails the catalog check.
- **Unicode / Normalized identifiers.** Still out of
  scope.
- **Quoted type names.** `"BIGINT"` is an identifier,
  not a type keyword.

## Evidence

- Source:
  `crates/bradian-relational/src/sql.rs` — new
  `Token::QuotedWord` variant; lexer branch for
  `"..."`; `Parser::word()` accepts both forms and
  lowercases.
- Tests:
  `crates/bradian-relational/tests/quoted_identifiers.rs`.
- ADR: `docs/adr/0181-quoted-identifiers.md`.
