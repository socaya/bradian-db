# ADR 0120 — CDC emitter primitive (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch L L2 — partial close
of BRD-MIG-002 scoped deviation.

## Scope

- `cdc::CdcEvent { Upsert{lsn,key,value} | Delete{lsn,key} }`.
- `CdcEmitter { last_lsn_emitted }`:
  - `emit_batch(lsn, mutations)` translates `RowMutation`s
    into events, dedupes by `lsn <= last_lsn_emitted`.
  - `resume_from(lsn)` moves watermark.

## Out of scope

- **WAL tail reader** — callers currently feed mutations
  from `commit_batch` / scan results. A durable
  resume-after-crash cursor is a follow-up.
- **Network push / pub-sub protocol**.
- **Schema-aware JSON projection** — consumers get raw
  key/value bytes.
- **Backpressure / ack protocol**.

## Compliance

- Batch translates both Put and Delete.
- LSN below watermark yields nothing.
- `resume_from` moves watermark.
- 4 unit tests.
