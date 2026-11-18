# ADR 0162 — In-process transport primitive (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-18.** Composes the full
BRD-WIRE-001 stack end-to-end so a future
`bradian-serving` crate can wrap it in a tokio listener
without redesigning the layering.

## 1. Scope

New module `local_transport`:

- `RequestPayload { Sql(String) | Prepared(PreparedStatement) }`.
- `ResponsePayload { Empty | Affected { rows } | Rows {
  columns, rows } }` — serde-serializable mirror of
  `DriverResponse`.
- `LocalTransport { driver: Arc<dyn Driver> }` with:
  - `handle_frame(request_frame) -> Response/Error frame`.
  - `handle_frame_chunked(request_frame, stream_id,
    chunk_size)` — splits large responses via ADR 0140
    chunk stream.
- `encode_request(payload)` / `decode_response(frame)`
  client helpers.
- Error translation: driver errors travel as Error
  frames with UTF-8 message bodies.

Composes:
- ADR 0137 wire-frame envelope (BRFR magic + CRC32).
- ADR 0139 `PreparedStatement` serde payload.
- ADR 0140 chunked response stream.
- ADR 0129 / 0158 / 0160 `Driver` trait + adapters.

## 2. Out of scope (procurement-dependent)

- **tokio listener + socket I/O** — belongs in
  `bradian-serving` crate; no runtime dependency
  burden on `bradian-relational`.
- **TLS/mTLS handshake** — needs cert issuance,
  rotation, and mutual-auth deployment infrastructure.
- **Session-aware transaction coordination over the
  wire** — needs the SessionDriver trait called out
  in ADR 0160.
- **Multi-client fairness / connection pooling**.
- **Frame batching** (multiple RPCs in one TCP
  packet).

## 3. Compliance

7 new unit tests in `local_transport::tests`:

- Round-trip plain-SQL request.
- Round-trip SELECT as Rows payload.
- Round-trip prepared-statement execution.
- Error frame returned for syntax error.
- Non-Request frame rejected with
  `TransportError::UnexpectedKind`.
- Transport cloneable, state shared through Arc.
- Chunked mode emits ≥1 frame for a small response.

## 4. Honest status

**BRD-WIRE-001** now has an in-process full-stack
transport. A tokio listener + TLS wrapper is the one
remaining BRD-WIRE-001 item, and it's
procurement-dependent — the stack composes cleanly
and only needs the OS-level networking glue.
