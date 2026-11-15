# ADR 0095 — Arc-shared dict bytes in `ColumnBuffer::DictIndex32` (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-15 and implemented.**

Closes the ADR 0082 §4(D) deferral. 50 pattern-match sites
touched across the workspace.

## 1. Context

Today `ColumnBuffer::DictIndex32` holds `dict_offsets:
Vec<u32>` and `dict_data: Vec<u8>` by value. Every clone of
the column (reader attach, concat, slice, etc.) copies the
full dict bytes. ADR 0087 reduced reader-side clones via a
single-pass fast path, but anything that calls
`ColumnBuffer::clone()` still pays the full copy.

## 2. Scope

**In scope:** replace both `Vec<_>` fields with `Arc<Vec<_>>`:
- `dict_offsets: Arc<Vec<u32>>`
- `dict_data: Arc<Vec<u8>>`

This keeps the on-the-wire types the same (`Arc<Vec<T>>`
derefs to `&[T]`; all slice-based access points keep working)
but makes `ColumnBuffer::clone()` cheap for dict columns and
makes multiple pages share one dict instance by default.

- Update all 50 pattern-match + construction sites.
- Keep `Clone`, `Debug`, `PartialEq`, `Serialize`,
  `Deserialize`. `Arc<Vec<T>>` implements all four via blanket
  impls on the inner `Vec`.

**Out of scope:**
- Changing the on-disk format (none).
- Changing the serde wire format (serde of `Arc<Vec<T>>` emits
  the same bytes as `Vec<T>`).
- Moving to `Arc<[T]>` or `Bytes` (potential future refinement).

## 3. Compliance

1. All 1137 pre-ADR-0095 tests pass unchanged.
2. Clippy clean.
3. Serde round-trip of a `DictIndex32` buffer via `bincode`
   produces byte-identical output to pre-ADR-0095.
4. `ColumnBuffer::clone()` on a dict column with a 1 MB dict
   does not copy the dict bytes (ref-count bump).

## 4. Alternatives considered

- **`Arc<[T]>`**: slightly smaller runtime footprint, but
  requires `Arc::from(vec)` conversion ergonomics. Vec wrap
  keeps call sites simpler.
- **`Bytes`** (external crate): rejected — new dep for no
  observable win here.

## 5. Acceptance evidence

1. ADRs 0060–0094 accepted.
2. 1137 workspace tests pass.
3. Clippy clean.
4. §3.3 serde round-trip test added.
