# ADR 0092 — Strict authenticated mode: extend to .brcs / BRWL / BRPG (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-15 and implemented.**

Follow-up to ADR 0091 (strict mode on the backup descriptor
only). Closes the remaining downgrade-attack surfaces on the
three other BACE carriers.

## 1. Scope

**In scope:** when `BaceWiring.strict_authenticated = true`
AND `authentication.is_some()`:
- `.brcs` reader rejects trailer `0` and trailer `1` **only
  on BACE-era segments** (`BACE_PAGE_ENVELOPE` feature flag
  set). Pre-BACE v1.0 segments carry no trailer byte, so
  strict mode cannot engage on them; operators who want to
  reject v1.0 entirely must check the segment minor
  version / feature flags before opening. Trailer `2` (BACA) and trailer `3`
  (BACE-with-dict, ADR 0079) remain accepted because they
  are not downgradable to a weaker shape (and trailer 3 has
  no BACA alternative yet).
- BRWL reader rejects opcode `4` (BACE batch). Opcodes
  `1`/`2` (single-op put/delete) and `3` (plain batch below
  threshold) stay readable: they have no authenticated
  alternative; rejecting them would make legitimate small
  writes unreadable.
- BRPG reader rejects bit-0 (BACE) pages in the data-page
  flag byte. v1 pages (flags == empty) stay readable since
  they have no authenticated alternative.

**Out of scope** (future ADRs):
- `.brcs` trailer 3 strict mode (needs BACA-with-dict first).
- BRWL opcode 3 strict mode (needs below-threshold BACA path).
- BRPG v1 strict mode (would require re-checkpointing every
  database before the flag takes effect).

## 2. Design

Each reader branch gets a one-line strict check right where
it dispatches on the trailer/opcode/flag. The error is
`Corruption` with "strict mode:" prefix, matching ADR 0091.

## 3. Compliance

1. Pre-ADR-0092 behavior preserved when `strict_authenticated
   = false`.
2. Strict + auth + .brcs trailer 0 → rejected.
3. Strict + auth + .brcs trailer 1 → rejected.
4. Strict + auth + .brcs trailer 2 → accepted (BACA).
5. Strict + auth + BRWL opcode 4 → rejected.
6. Strict + auth + BRWL opcode 5 → accepted.
7. Strict + auth + BRPG bit-0 page → rejected.
8. Strict + auth + BRPG bit-1 page → accepted.
9. 1122 pre-ADR-0092 tests pass.
10. Clippy clean.
