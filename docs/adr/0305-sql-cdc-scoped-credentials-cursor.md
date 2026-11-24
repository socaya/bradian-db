# ADR 0305 — SQL CDC scoped credentials + cursor persistence

## Status

Accepted. Fifth slice of P1-4, closing P1-4.

## Context

`bradian_sources::sql_cdc` already defines a trait + row
shape + canonical envelope projection for a pull-only CDC
adapter. The P1-4 acceptance for this slice requires two
operational properties:

1. **Admin credentials scoped** — the connection the
   adapter uses must be least-privilege (read / replication
   only), with a typed representation that refuses an
   obviously over-powered credential at construction time.
2. **Cursor persistence** — the last-applied LSN must
   survive restart so a resumed run does not replay rows
   that have already been ingested.

## Decision

Additive, inside `bradian_sources::sql_cdc`:

1. `SqlCdcCredentials { role, replication_slot, readonly }`
   constructed via `try_new`. `readonly=false` is
   refused with a typed error — the only legitimate CDC
   credential is read-only.
2. `CursorStore` persists the last-applied LSN to a single
   file via atomic rename (`write(lsn)` writes
   `<path>.tmp` then renames over `<path>`). `load`
   returns `0` if the file does not exist, so a fresh
   adapter starts from zero without a pre-seeded state
   file.
3. `BoundedCdcPuller<S>` wraps any `SqlCdcSource`, loads
   the cursor on construction and persists after each
   `next_batch` returns rows (max observed LSN across the
   batch). Its `start_from` surfaces the cursor so the
   underlying source can seek.

Scope limits:

- `SqlCdcCredentials` is an advisory structural type: it
  validates shape, not the actual database grants (the
  caller still has to hand it to a Postgres driver that
  honours them). Full server-side enforcement is a
  future transport slice.
- Cursor persistence is per-source and in-process only;
  concurrent writers to the same cursor file race on the
  atomic rename. The adapter is single-reader by design.
- No live Postgres client in this slice — that's a
  transport slice under a feature flag once reqwest /
  tokio-postgres is approved. Tests use an in-process
  mock implementing `SqlCdcSource`.

## Consequences

- A production operator can wire a read-only replication
  slot into the adapter and get restart-safe pull
  semantics without additional glue.
- 4 new unit tests: credentials refuse `readonly=false`;
  cursor file round-trips zero / non-zero LSN;
  `BoundedCdcPuller` resumes from the persisted cursor;
  the cursor advances across successive batches.
- No change to `SqlCdcRow`, `rows_to_changes`, or the
  canonical envelope shape.
