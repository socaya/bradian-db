# ADR 0140 — Chunked response stream primitive (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Companion to ADR 0137
(wire-frame envelope). Enables payloads larger than
MAX_PAYLOAD (16 MiB) and streaming result sets.

## 1. Scope

New module `bradian_relational::chunk_stream`:

- `Chunk { stream_id: u64, seq: u32, total: u32,
  body: Vec<u8> }` — serde-serialized.
- `split(payload, stream_id, chunk_size) -> Vec<Chunk>`
  — fragmentation. Empty payload yields a single marker
  chunk so the receiver always sees a stream completion.
- `Assembler` — stateful reassembler; `push(chunk)` +
  `is_complete()` + `take()`.
- `ChunkError` variants: `InvalidChunkSize` /
  `PayloadTooLarge` / `StreamIdMismatch` /
  `TotalMismatch` / `DuplicateSeq` / `SeqOutOfRange` /
  `Incomplete`.

Behavior:
- Chunks arrive in any order; `Assembler` stores by seq
  and reorders on `take()`.
- Mismatched `total` across chunks rejected.
- Mismatched `stream_id` across chunks rejected.
- Duplicate seq rejected.
- `seq >= total` rejected.
- `take()` before completion returns `Incomplete`.

## 2. Out of scope (follow-ups)

- **Real wire-frame wrap** — caller composes with
  ADR 0137 `encode`/`decode`, one Chunk per frame.
- **Backpressure / credit protocol** — assembler holds
  all received chunks in memory until complete; no
  flow control.
- **Cancellation / reset** — no `cancel_stream(id)`
  path.
- **Interleaved streams on one connection** — a
  separate `Assembler` per `stream_id` is the caller's
  responsibility.
- **Compression per chunk** — bodies are opaque bytes.
- **Resumability** — a dropped mid-stream connection
  loses all received chunks for that stream_id. A
  future ADR can add durable sequence resumption.

## 3. Compliance

- Small payload → single chunk with `total = 1`.
- Payload at chunk boundary → correct fragmentation,
  seqs in order.
- Empty payload yields one marker chunk.
- Invalid chunk_size (0 or > MAX_PAYLOAD) rejected.
- Assembler round-trips any payload.
- Out-of-order push → ordered concatenation on take.
- Stream id mismatch rejected.
- Duplicate seq rejected.
- Total mismatch rejected.
- seq ≥ total rejected on push.
- `take()` pre-completion → `Incomplete`.
- serde JSON round-trip preserves exactly.
- 14 new tests in `chunk_stream::tests`.

## 4. Honest status

Foundation only. **BRD-WIRE-002 streaming row delivery**
is not yet satisfied because no transport mounts chunked
responses, no backpressure/credit protocol exists, no
cancellation path ships, and no engine streams rows.
The primitive is the prerequisite that future network
driver work consumes.
