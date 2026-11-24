# ADR 0255 — Fault-injection sweep on user and DDL journals

## Status

Accepted.

## Context

ADR 0254 landed the property sweep (byte-flip and tail-
truncation across many offsets) for the RowStore WAL. The
same property applies verbatim to the user-catalog and
DDL-catalog journals — they are both built on `RowStore` and
replay through journal-specific validators — but no sweep
exercised them. This ADR closes that symmetric gap.

## Decision

Apply the same strong property to each of
`PersistentUserCatalog::rows.wal` and
`PersistentCatalog::rows.wal`:

> For any fault applied to a seeded journal, the reopened
> journal either returns an explicit error, or exposes a
> state that is a subset of the originally committed state
> (every present entry was in the seeded set, with values
> matching).

For users, "values matching" means the stored password still
verifies against the original plaintext. For the DDL catalog,
"subset" is checked against a known allowlist of seeded
database and table names so any fabrication surfaces.

Sweep shape is identical to ADR 0254:
- Byte-flip: ~32 sampled offsets (header, evenly spaced
  middle, tail region).
- Tail truncation: 20 evenly spaced points + every byte in
  the last 8.

Error allowlists are explicit per journal:
- `PersistentUserCatalogError::{Storage, Corruption, Encoding}`.
- `PersistentCatalogError::{Storage, Corruption, Encoding, Catalog}`
  (the DDL journal's replay may reject an event through a
  `CatalogError`, surfaced as `PersistentCatalogError::Catalog`).

## Consequences

- P0-5 swept-property coverage now spans all three
  `RowStore`-backed journals.
- Confirms higher journals continue to propagate
  `Storage`/`Corruption` through their own error variants,
  across the full swept space — not just the three offsets
  pinned in ADR 0253.
- Finishes in ~10s (the argon2id KDF in `create_user` is the
  dominant cost; the per-scenario reopen is cheap).
- Scope: sweep-shape identical to ADR 0254. A `FaultPolicy`
  trait with runtime hooks (fail at the Nth write) and
  workload-level sequencing remain future slices.
- 4 tests in `tests/fault_injection_sweep_journals.rs`:
  user-byte-flip, user-tail-truncation, catalog-byte-flip,
  catalog-tail-truncation.
