# ADR 0127 — in-memory non-unique secondary index helper (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch D D2 primitive.
BRD-IDX-002 "non-unique secondary indexes" is OPEN;
D1 (unique secondary enforcement) was already seeded by
ADR 0101; D3 (reader fuzzing) and D4 (auto-PITR wiring)
remain follow-ups.

## 1. Scope

New module `bradian_relational::secondary_index`:

- `NonUniqueIndex` — `BTreeMap<Vec<u8>, Vec<Vec<u8>>>`
  mapping encoded secondary-key bytes to one or more
  primary-key byte blobs.
- `insert(sk, pk)` — idempotent per exact `(sk, pk)`
  pair.
- `delete(sk, pk) -> bool` — removes a single edge;
  cleans up empty buckets.
- `lookup(sk) -> &[Vec<u8>]` — exact point lookup.
- `range(low, high)` — sorted iterator `(&sk, &pk)`;
  within an equal secondary-key bucket preserves
  insertion order.
- `len()` / `distinct_keys()` / `is_empty()`.

## 2. Out of scope (follow-ups)

- **Disk-backed storage / WAL integration** — this is
  in-memory only. Durable secondary indexes require the
  mutable-storage work on top of ADR 0096 (Gate 3).
- **Concurrent writers** — callers hold an external
  lock. A future `ConcurrentNonUniqueIndex` can wrap
  `parking_lot::RwLock`.
- **Catalog `CREATE INDEX` DDL** — the index doesn't
  exist in the catalog. The Catalog has `unique_keys`
  but no general-purpose `Index` object yet.
- **Secondary-key encoding** — callers pass bytes. For
  lexicographic numeric order use big-endian /
  sign-bit-flipped encodings (same contract as the
  checkpoint B+tree).
- **Range-scan admission / resource accounting** — the
  iterator is unbounded; ADR 0041 scan-admission would
  apply to a wired-in version.
- **Query-planner integration** — the SQL engines have
  no query planner yet. A predicate `WHERE col =
  <literal>` does not consult this index automatically.

## 3. Compliance

- Insert/lookup/delete round-trip for single edges.
- Multiple PKs mapped to one SK coexist.
- Idempotent insert.
- Delete-last cleans up the bucket (distinct_keys
  returns to 0).
- Delete-nonexistent returns false.
- Range scan sorted by secondary key.
- Range bounds (Included / Excluded / Unbounded)
  respected.
- Within-bucket order is insertion order.
- Empty bounded range (`[a, a)`) yields nothing.
- 11 new unit tests in `secondary_index::tests`.

## 4. Honest status

Foundation only. **BRD-IDX-002** is not yet satisfied
because there is no catalog object, no WAL
integration, no concurrent writer support, and no
query-planner wiring. The primitive is a prerequisite
that a future ADR can integrate.
