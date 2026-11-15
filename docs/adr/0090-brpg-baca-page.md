# ADR 0090 — BRPG v2 BACA page (authenticated checkpoint snapshot pages) (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-15 and implemented.**

Blanket pre-approval from the owner to continue the BACE gap
cycle. Third ADR in the 0089–0091 batch.

## 1. Context

- ADR 0065 defined BRPG v2 pages carrying BACE envelopes in
  the `payload_is_bace_envelope` flag (bit 0).
- Those pages are integrity-protected by the BRPG 32-byte
  header's CRC32 and by the BACE envelope's own CRC, but
  **not authenticated**. An attacker with write access to
  snapshot files can swap page bytes (same CRC) or forge a
  page that reads back as valid data.
- ADR 0080 integrated BACA (authenticated BACE variant) into
  `.derc` pages via trailer byte 2. ADR 0086 did the same
  for BRWL batches (opcode 5). The checkpoint snapshot
  pages remain the last unauthenticated BACE carrier.

## 2. Scope

**In scope:**

- Add BRPG v2 flag bit 1: `payload_is_baca_envelope`. When
  set, the page payload is exactly one BACA envelope (ADR
  0075); the reader verifies MAC under the wiring's
  `BaceAuthentication` before decoding.
- Writer: when wiring has `authentication.is_some()`,
  `emit_data_page` produces a BACA-wrapped page (bit 1) in
  place of a BACE-wrapped page (bit 0).
- Reader (`decode_data_page_payload`): dispatch on both
  flags; BACA needs an authentication key. Hard-fail on
  missing auth or MAC mismatch.
- The pages' capacity budget shrinks by 16 bytes under BACA
  to accommodate the MAC trailer.
- Reserved-bits check: v2 was `0b1111_1110`; now
  `0b1111_1100` (bits 0 and 1 known).
- Tests covering writer flag, reader round-trip, wrong-key
  reject, missing-auth reject.

**Explicitly out of scope** (future ADRs):

- MAC over the BRPG header itself. CRC32 catches bit rot;
  the page MAC authenticates the content.
- Streaming decryption. BACA is integrity-only.
- Migration from bit-0 (BACE) to bit-1 (BACA) in existing
  snapshot files. Operators who want BACA re-checkpoint.

## 3. Design

### 3.1 Flag layout

```rust
pub struct PageFlags {
    pub payload_is_bace_envelope: bool,  // bit 0 (ADR 0065)
    pub payload_is_baca_envelope: bool,  // bit 1 (ADR 0090)
}
```

Mutually exclusive by construction: setter prevents both
being true.

### 3.2 Writer

`emit_data_page` takes an extra `auth: Option<&BaceAuthentication>`.
When `Some`, the plain bytes are encoded under the codec,
wrapped in a BACA envelope (via
`write_authenticated_envelope`), and emitted as a BRPG v2
page with `payload_is_baca_envelope = true`.

The row_store's `write_snapshot` extracts auth from the
wiring and passes it to `emit_data_page`.

### 3.3 Reader

`decode_data_page_payload` branches:
- No flags set → return raw payload (v1 or legacy v2).
- `payload_is_bace_envelope` → existing BACE path.
- `payload_is_baca_envelope` → new BACA path:
  - Require `wiring.authentication.is_some()`.
  - `read_authenticated_envelope` verifies MAC.
  - Decode via codec.
- Both flags set → `Corruption` (mutually exclusive).

### 3.4 Compliance

1. No auth wiring: writer emits bit-0 (BACE) pages as
   today. Existing ADR 0065 round-trip tests pass.
2. Auth wiring: writer emits bit-1 (BACA) pages.
3. Round-trip under matching auth key works.
4. Wrong key → reader reports MAC mismatch; page rejected.
5. Missing auth on a BACA page → reader rejects with clear
   error.
6. Tampered BACA payload → MAC mismatch.
7. Both flags set in a hand-crafted page →
   `UnsupportedFeatureFlag` or `Corruption`.
8. Index recovery still works through the new decode path.
9. Clippy clean.
10. 1111 pre-ADR-0090 workspace tests pass.

## 4. Alternatives

**A. Overload bit 0 with a sub-flag in the envelope.**
Rejected: puts semantics in the envelope that affects BRPG
validation. Keeping it in the BRPG flags byte stays
consistent with ADR 0065.

**B. Put the MAC in the BRPG header.** Rejected: changes
the 32-byte BRPG header, breaking binary compat with
existing v1/v2 tools.

**C. MAC the whole 4096-byte page.** Rejected: the BRPG
header has its own CRC; a payload-scoped MAC matches BACE/
BACA semantics on other carriers (.derc, BRWL).

## 5. Open design questions

1. **Mutual exclusion enforcement.** Draft: writer
   constructor rejects both-flags-set; decoder also rejects.
   Confirm.
2. **Flag naming.** Draft: `payload_is_baca_envelope`.
   Alternatives: `payload_authenticated`. Keep draft.

## 6. Acceptance evidence

1. 1111 pre-ADR-0090 tests pass.
2. New §3.4 integration tests.
3. Clippy clean.
