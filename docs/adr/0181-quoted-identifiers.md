# ADR 0181 — Quoted identifiers (`"name"`) in the SQL lexer (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-21.** Closes the ADR 0113 §2
and ADR 0112 §2 follow-up:

> **Quoted identifiers** (`"MyTable"`) — follow-up.

## 1. Scope

- **Lexer**: new `Token::QuotedWord(String)` variant. `"..."`
  is parsed as an identifier token; `""` inside a quoted
  identifier is an escaped double-quote, mirroring the
  SQL-standard `''` escape in string literals.
- **Parser**: `Parser::word()` accepts both `Token::Word`
  and `Token::QuotedWord`. Both normalize to lowercase at
  the boundary to match the catalog's ADR 0006 case-
  insensitive storage rule.
- **Keyword matching** (`consume_word` / `next_is_word`):
  **only** matches `Token::Word`. Quoted identifiers are
  never recognized as keywords, so `"SELECT"` can name a
  user column without colliding with the reserved word.
- Empty quoted identifier (`""`) is rejected at lex.
- Unterminated quoted identifier is rejected at lex with a
  dedicated message.
- The catalog's existing `identifier()` validation
  (ASCII-alphanumeric + underscore, non-empty, ≤63 bytes)
  still applies post-lex, so content that passes the
  lexer but violates the catalog rules surfaces as
  `SqlError::Catalog`.

## 2. Out of scope

- **Case-preserving identifier semantics.** The lexer
  preserves case in the token, but the parser lowercases
  at the identifier boundary to match ADR 0006. A future
  ADR that revises the catalog's case policy could trivially
  stop lowercasing in `word()`; the Token variant is
  already distinct.
- **Non-ASCII quoted content.** The catalog still requires
  ASCII identifiers; `"éclair"` lexes but fails the
  catalog check.
- **Unicode / Normalized identifiers.** Out of scope.
- **Quoted type names.** `"BIGINT"` would be treated as an
  identifier, not the BIGINT type keyword — rejected by
  `data_type()` as an unsupported type.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/quoted_identifiers.rs`
(7 tests, all green):

1. `quoted_identifier_in_create_table_name_is_accepted` —
   `CREATE TABLE "t"."public"."items"` works.
2. `quoted_identifier_can_hold_a_reserved_word_as_column_name`
   — `"select" TEXT NOT NULL` round-trips; columns can be
   referenced only via the quoted form.
3. `unquoted_and_quoted_names_resolve_to_the_same_catalog_object`
   — `"T"."PUBLIC"."ITEMS"` and `t.public.items` map to
   the same row.
4. `escaped_double_quote_inside_quoted_identifier_is_parsed`
   — `"has""quote"` lexes; catalog rejects because the
   content violates the identifier charset (expected).
5. `empty_quoted_identifier_is_rejected_at_lex`.
6. `unterminated_quoted_identifier_is_rejected_at_lex`.
7. `quoted_identifier_does_not_match_keyword_in_consume_word_path`
   — `"primary" key (id)` as a column list fails cleanly;
   the quoted `"primary"` did not bind the `PRIMARY KEY`
   clause.

Clippy clean on `-D warnings`.

## 4. Roadmap impact

Closes two §2 follow-ups:

- ADR 0112 §2 "Quoted identifiers" — "would enable case-
  preservation" is now honestly scoped: the slice closes
  the **keyword-reservation** value; case preservation is
  deferred to a catalog-policy ADR.
- ADR 0113 §2 "Quoted identifiers" — follow-up delivered.

Remaining ADR 0113 §2 items: JSON path operators (deferred
with ADR 0173 §2), FOREIGN KEY action keywords, CHECK
constraint parsing, prepared named parameters.
