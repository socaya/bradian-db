# ADR 0097 — Online-DDL reader-session schema pin (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-15 as a Gate-1-scope foundation.**

## 1. Honest scope

Existing behavior already provides "readers hold their schema":
`PersistentCatalog::snapshot()` returns an immutable `Catalog`
copy. DDL creates a new revision; old snapshots keep reading
the old schema. This is correct but INVISIBLE — operators
have no way to see which schema revisions are still pinned,
and the test matrix did not lock in the "DDL does not
invalidate active snapshots" invariant.

This ADR delivers a thin operator-visible pin registry:

- `PersistentCatalog::reader_session() -> ReaderSession` —
  returns an RAII handle that holds a `Catalog` snapshot and
  registers its revision.
- `ReaderSession::revision()` and `ReaderSession::snapshot()`.
- `Drop` for `ReaderSession` deregisters the pin.
- `PersistentCatalog::active_reader_revisions() -> BTreeMap<u64, usize>`
  — a one-shot read of current pin counts per revision.
- Tests proving: (a) DDL during an active session does NOT
  invalidate the session's view; (b) the registry reflects
  session lifetimes; (c) multiple sessions at different
  revisions coexist.

## 2. Out of scope

- **Blocking DDL on active readers.** A future ADR could use
  `active_reader_revisions` to reject DDL that would strand
  readers; this ADR is diagnostic only.
- **Concurrent-write DDL** — still single-process, single-
  writer. The claim in CLAUDE.md §2 "concurrent online DDL …
  remain incomplete" holds.
- **Cross-process reader coordination** — the pin registry
  is in-process. Multi-process online DDL is Gate 4+ territory.
- **Row-level MVCC during DDL** — handled by
  `durable_mvcc`/`history_sql` under their own ADRs.

## 3. Compliance

1. A `ReaderSession` holds a `Catalog` snapshot at a specific
   revision. The snapshot is immutable; DDL on the live
   catalog does not mutate it.
2. `active_reader_revisions()` returns every live pin count.
3. `Drop` on a session decrements the count; when the count
   hits zero, the revision key is removed.
4. Multiple sessions at the same revision coalesce into one
   key with count > 1.
5. Tests covering §3 items 1–4.
6. Clippy clean.
