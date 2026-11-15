# ADR 0086 — BRWL BACA carrier (authenticated WAL batch frames) (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-15 and implemented.**

Blanket pre-approval from the owner to continue the draft+
implement cycle for ADRs 0086–0088.

## 1. Context

- BRWL (bradian relational WAL) today carries four opcodes:
  `1` = single put, `2` = single delete, `3` = plain batch,
  `4` = BACE-wrapped batch (ADR 0077).
- `.derc` segments gained authenticated pages (BACA) in ADR
  0080. `BaceWiring.authentication: Option<BaceAuthentication>`
  is set once per store.
- BRWL batches above the `BRWL_BACE_MIN_BODY_BYTES` threshold
  wrap in BACE (opcode 4) when wiring resolves a non-RAW
  codec. Those batches are **not authenticated today** — a
  tampered WAL frame would pass CRC32 but silently restore
  wrong data.
- Backup archive adapter (ADR 0067) and RowStore checkpoint
  pages are NOT addressed in this ADR; see §2 out-of-scope.

## 2. Scope of this ADR

**In scope:**

- Add BRWL opcode `5` = BACA-wrapped batch (authenticated
  BACE variant). Same body layout as opcode 4 but the
  envelope carries a BLAKE3-128 MAC over
  `(codec_id || codec_format_version || flags ||
    encoded_length || decoded_length || encoded_bytes)`.
- Writer dispatch: when `BaceWiring.authentication` is set
  AND the batch would otherwise hit opcode 4, emit opcode 5
  instead. Below-threshold batches still emit opcode 3
  (unchanged).
- Reader dispatch: opcode 5 → decode via BACA (MAC verify
  under wiring's authentication key before decoding).
  Hard-fail with `StoreError::Corruption` on MAC mismatch.
- Threshold `BRWL_BACE_MIN_BODY_BYTES` unchanged — BACA adds
  16 bytes of MAC + 36 bytes of envelope header = 52 bytes
  overhead per batch. Below the threshold, the overhead
  outweighs any compression benefit, so stay in opcode 3.
- Tests: round-trip under matching key, wrong-key reject,
  tampered-envelope reject, downgrade attempt (opcode 4
  frame submitted to a reader that has authentication)
  rejected if a policy flag is set (see §5 Q3).

**Explicitly out of scope** (future ADRs):

- **Backup archive BACA.** ADR 0067 wraps backup frames in
  BACE; adding BACA there is a separate ADR with its own
  threat-model section.
- **RowStore checkpoint-page BACA.** Checkpoints are large
  snapshot artifacts; MAC coverage is a separate ADR.
- **Downgrade protection.** This ADR does NOT reject opcode
  4 frames when wiring has authentication (so a legacy WAL
  tail remains readable after key rotation). §5 Q3 proposes
  a strict mode as a follow-up ADR.
- **Key rotation workflow.** Separate ADR.
- **CLI exposure for operators to enforce BACA on BRWL.**
  Separate ADR.

## 3. Proposed design

### 3.1 Writer

`encode_batch_with_optional_bace`:
- Pre-ADR-0086 dispatch unchanged: below threshold → opcode
  3; above threshold + non-RAW codec → opcode 4.
- NEW: when `wiring.authentication.is_some()` AND dispatch
  would have been opcode 4 → emit opcode 5 via
  `wrap_in_baca_envelope`.

The BACA envelope uses `bace::authenticated::
write_authenticated_envelope`, which is already available
from ADR 0075.

### 3.2 Reader

`decode_frame_body` (opcode dispatch):
```rust
5 => {
    let wiring = bace.ok_or_else(|| /* corruption */)?;
    let auth = wiring.authentication.as_ref().ok_or_else(|| /* corruption */)?;
    let envelope_bytes = &body[9..];
    let decoded = decode_baca_wal_envelope(envelope_bytes, wiring, auth, offset)?;
    decode_batch_payload(&decoded, offset)
}
```

New helper `decode_baca_wal_envelope` mirrors
`decode_bace_wal_envelope` but calls
`read_authenticated_envelope` with the key, returning
`StoreError::Corruption` on MAC mismatch.

### 3.3 Compliance obligations

1. Below-threshold batches still emit opcode 3 under any
   wiring.
2. Above-threshold + non-auth wiring still emits opcode 4
   (byte-identical to pre-ADR-0086).
3. Above-threshold + auth wiring emits opcode 5 with a
   valid MAC.
4. Reader round-trips opcode 5 batches under matching key.
5. Wrong-key reader rejects opcode 5 with `Corruption`
   mentioning MAC verification.
6. Tampered envelope (flip a payload byte) → `Corruption`.
7. Opcode 5 frame submitted to a reader without
   authentication → `Corruption` (clear error message).
8. Existing ADR 0077 opcode-4 round-trip tests pass
   unchanged.
9. Reopen a store after a crash mid-opcode-5 write:
   truncated frame is dropped like any other torn WAL
   frame.
10. 1089 pre-ADR-0086 tests pass unchanged.

## 4. Alternatives considered

**A. Reuse opcode 4, add a flag byte for auth.** Rejected:
mixes concerns in one opcode and complicates version
negotiation.

**B. MAC over the whole frame including CRC.** Rejected:
CRC is already verified first; MAC covers the envelope
contents, which is sufficient against tampering of the
decoded body.

**C. Encrypt the batch body.** Out of scope; this ADR
provides integrity (BACA = authenticated), not
confidentiality.

## 5. Open design questions

1. **Opcode number.** Draft: `5`. Confirm no reservation.
2. **Below-threshold BACA.** Draft: no — small batches
   stay opcode 3 to avoid overhead. Alternative: force
   BACA for all batches under auth wiring. Reject per §3.
3. **Downgrade protection.** Draft: NOT in scope; opcode
   4 frames remain readable under auth wiring. Future ADR
   can add a strict mode.
4. **Error variant.** Draft: `StoreError::Corruption`
   mentioning MAC verification. Alternative: dedicated
   variant. Keep Corruption for consistency with ADR 0080.

## 6. Acceptance evidence

1. ADRs 0060–0085 accepted — satisfied.
2. §3.3 items 1–10 covered.
3. Workspace tests all pass.
4. Clippy clean.

Implementation delivered per the owner's blanket "continue
all three" directive.
