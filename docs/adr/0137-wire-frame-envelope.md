# ADR 0137 — Wire-frame envelope primitive (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Companion to ADR 0129
(`Driver` trait). Defines the on-wire packet format a
future network driver will use between client and server.

## 1. Scope

New module `bradian_relational::wire_frame`:

- Fixed 16-byte header + opaque payload + trailing CRC32.
- `encode(kind, payload) -> Vec<u8>` — complete frame.
- `decode(bytes) -> (FrameKind, Vec<u8>)` — strict
  validation.
- `FrameKind { Request / Response / KeepAlive / Error }`.
- `FrameError` with distinct variants for each failure
  mode (TooShort, BadMagic, UnsupportedVersion,
  UnknownKind, PayloadTooLarge, LengthMismatch,
  ChecksumMismatch).

Frame layout (big-endian):

```
offset  size  field
------  ----  -----
 0       4    MAGIC "BRFR"
 4       2    version (u16, currently 1)
 6       1    kind (u8)
 7       1    reserved (ignored on decode)
 8       4    payload_len (u32)
12       4    reserved CRC placeholder (zeroed before checksum)
16    N≤MAX   payload
16+N     4    CRC32/ieee over bytes[0..16+N] with the
              placeholder window zeroed
```

Bounds: `MAX_PAYLOAD = 16 MiB`.

## 2. Out of scope (follow-ups)

- **TLS / mTLS transport** — this is the in-stream
  envelope only. BRD-SEC-003 applies at the connection
  layer.
- **Compression** — payload bytes are opaque; a caller
  can pre-compress.
- **Fragmentation** — frames carry up to MAX_PAYLOAD
  bytes in one packet. Larger logical messages must
  split (a future ADR defines the chunking protocol).
- **Authentication** — BACE / BACA envelopes apply at a
  higher layer and travel inside the payload.
- **Version negotiation** — `ver == 1` only. Downgrade
  logic is a follow-up.
- **Streaming row delivery** — Response frames carry one
  complete payload; a future `ResponseChunk` kind will
  support streaming result sets.

## 3. Compliance

- Round-trip for every frame kind.
- Empty payload round-trips.
- Too-short input rejected.
- Bad magic rejected.
- Unsupported version rejected.
- Unknown kind rejected.
- Length mismatch rejected (header says one length, body
  actually different).
- Bit flip anywhere in payload detected via CRC32.
- Oversize payload rejected on both encode and decode.
- Reserved byte is ignored on decode (verified by
  tampering with recomputed CRC).
- 12 new tests in `wire_frame::tests`.

## 4. Honest status

Foundation only. **BRD-WIRE-001 portable driver wire
protocol** is not yet satisfied because no transport,
no auth, no negotiation, no streaming responses, no TLS
and no actual network driver exist. The envelope is the
prerequisite that a future `bradian-wire` crate consumes.
