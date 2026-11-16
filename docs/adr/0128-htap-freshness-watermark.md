# ADR 0128 — HTAP freshness watermark primitive (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch E E2 primitive.
E1 (committed columnar projection) and E3 (vectorized
reads) remain follow-ups. The watermark is the freshness
contract a future HTAP coordinator can use to answer
"has columnar caught up?" without owning the projector
itself.

## 1. Scope

New module `bradian_relational::htap_watermark`:

- `HtapWatermark` — thread-safe `Arc`-shared struct
  holding two `AtomicU64`s: `latest_committed` (OLTP
  side) and `latest_projected` (OLAP side).
- `advance_committed(lsn) -> bool` — monotone CAS; a
  lower value is a silent no-op.
- `advance_projected(lsn) -> bool` — monotone CAS;
  additionally rejects `lsn > latest_committed` (the
  columnar side cannot claim it has caught up to a
  commit that hasn't happened).
- `lag() -> u64` — `committed - projected`, saturating.
- `is_fresh_as_of(lsn) -> bool` — `projected >= lsn`.
- `snapshot() -> WatermarkSnapshot` — paired read; two
  loads so the two numbers may race but each is
  monotone.
- `Clone` shares the same inner.

## 2. Out of scope (follow-ups)

- **Columnar projector** — this is a bookkeeping
  primitive. The actual catch-up worker lives in the
  analytical engine; it drives `advance_projected`.
- **Per-table / per-partition watermarks** — a single
  global pair for now. A future primitive can hold a
  BTreeMap<TableId, HtapWatermark>.
- **LSN-to-wall-clock conversion** — operators will
  want "fresh within 500 ms"; that requires a WAL
  LSN-timestamp map.
- **Wait-until-fresh blocking** — poll-based only.
  Blocking requires a parking_lot condvar wrap.
- **Admission to analytical reads based on
  freshness** — the SQL engine doesn't consult this
  watermark today; callers who care about HTAP
  freshness must poll explicitly.

## 3. Compliance

- Starts at (0, 0).
- Committed advances monotonically; backward advance
  rejected.
- Projected rejected when > committed.
- Projected catches up to committed → lag == 0.
- `lag()` reports commit − projected.
- `is_fresh_as_of(lsn)` matches `projected >= lsn`.
- `snapshot()` captures both sides.
- `Clone` shares state.
- Concurrent 8-thread advance preserves monotonicity
  and `projected <= committed`.
- 10 new tests in `htap_watermark::tests`.

## 4. Honest status

Foundation only. **BRD-HTAP-002 committed columnar
projection contract** is not yet satisfied because
there is no projector, no per-table identity, no
automatic admission and no wall-clock conversion.
The watermark is a prerequisite that a future ADR can
integrate into both engines.
