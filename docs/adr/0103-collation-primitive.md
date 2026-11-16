# ADR 0103 — Collation primitive (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16 as a comparison primitive.**

## 1. Honest scope

`bradian_relational::collation::Collation` with two variants:

- `Binary` (default) — bitwise comparison. Zero-cost; matches
  every existing ORDER BY / UNIQUE comparison today.
- `CaseInsensitive` — ASCII case folding. Non-ASCII bytes
  compare bytewise.

API:
- `compare(&self, a, b)` → `std::cmp::Ordering`
- `sort_key(&self, s)` → `Vec<u8>` for stable sort caching
- `eq(&self, a, b)` → `bool`

## 2. Out of scope

- **Full Unicode UCA / locale-aware collation.** Requires
  ICU. Future ADR.
- **Catalog storage / DDL syntax** — no `COLLATE` clause in
  CREATE TABLE yet. Operators use this primitive directly
  in-code.
- **BRD-SQL-006 compliance beyond ASCII**.

## 3. Compliance

1. Default is `Binary`.
2. `Binary::compare` matches `str::cmp`.
3. `CaseInsensitive` treats `"Alice"` and `"alice"` as equal.
4. `sort_key` preserves the compare order under
   `sort_by_key`.
5. Non-ASCII bytes compare bytewise (documented honest
   boundary).
6. Length ties: shorter sorts first.
7. 6 unit tests in `src/collation.rs::tests`.
