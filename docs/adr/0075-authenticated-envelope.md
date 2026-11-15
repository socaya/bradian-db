# ADR 0075 — BACE authenticated envelope (primitive; **PROPOSED**)

**Status: PROPOSED — not accepted for implementation.**

Blocked on the accepted BACE workstream through ADR 0074. This
ADR adds a new envelope family under the `BACA` magic that ADR
0062 reserved. It is bounded to the **envelope primitive only**:
an authenticated byte-level wrapper with a keyed MAC tag.
Integration into any carrier (`RowStore` checkpoint writer,
backup archives, `.derc`, BRWL) is explicitly deferred to future
ADRs so the primitive can ship with a tight scope and clear
threat model.

Owner review required before any code lands per CLAUDE.md §4.9
and §6.

## 1. Context

- ADR 0062 reserved the `BACA` magic for a future
  authenticated envelope family. BACE v1 (magic `BACE`) uses
  CRC32 only — it detects accidental damage, not malicious
  tampering.
- CLAUDE.md §6 requires "every production security claim
  needs evidence of isolation, revocation, audit integrity,
  key rotation, backup protection and incident response."
  **This ADR deliberately does NOT make a production security
  claim.** It adds a library primitive. Making a production
  claim requires a separate carrier-integration ADR that
  comes with the matching evidence.
- Threat model for the envelope primitive itself: a reader
  holding a 32-byte key must detect any byte-level tampering
  of the envelope header or payload before the payload is
  decoded.
- Non-goals (explicit): this primitive does NOT provide
  confidentiality (payload is not encrypted), does NOT bind
  the envelope to a specific carrier file (that is the
  carrier's job), does NOT handle key provisioning, rotation,
  revocation, or storage, and does NOT prevent replay (that
  needs an anti-rollback counter in a carrier).
- Non-goals (explicit, library-wide): this ADR does NOT
  implement any cryptographic algorithm. It uses BLAKE3's
  audited keyed-hash mode via the existing `blake3`
  workspace dependency (already used by `dap-security`).

## 2. Scope of this ADR

**In scope:**

- New module `bace::envelope::authenticated` in
  `crates/bradian-relational/src/bace/envelope.rs` (or a
  sibling file; implementation detail).
- `AuthenticatedEnvelopeHeader` struct mirroring the BACE v1
  header but with different magic and reserved bytes.
- `AuthenticatedEnvelopeFlags` struct identical in shape to
  `EnvelopeFlags`.
- On-disk layout: 32-byte header, variable-length payload,
  4-byte trailer CRC32, 16-byte MAC tag. Total overhead:
  52 bytes vs BACE v1's 36 bytes. +16 bytes per envelope.
- `write_authenticated_envelope(output, header, payload, key)`
  — mirror of `write_envelope`; computes MAC over header
  bytes + payload bytes.
- `read_authenticated_envelope(input, caps, key)` — mirror of
  `read_envelope`; validates MAC **before** returning the
  payload to the caller for decode.
- `AuthenticatedEnvelopeError` enum covering: MAC mismatch,
  short key (non-32-byte), magic mismatch, version mismatch,
  any ADR 0062 §3.3 bound violation.
- MAC primitive: BLAKE3 keyed hash via `blake3::keyed_hash`,
  with 32-byte key and 16-byte truncated tag.
- Constant-time tag comparison via `subtle` (already a
  transitive dep of workspace crypto libs; confirm).
- `BaceAuthentication` type: `{ key: [u8; 32], label: String }`
  for later wiring integration. The label is operator-facing
  text used only in error messages and logs; it is NOT bound
  into the cryptographic computation.
- 10+ unit tests covering round-trip, every documented
  failure mode, and the critical constant-time-comparison
  property (test that the MAC check uses `subtle`).

**Explicitly out of scope** (future ADRs):

- **RowStore / checkpoint writer integration.** No BACA
  envelopes get produced by any existing carrier. The
  `RowStore::open_with_bace` path and
  `backup_to_with_bace` remain BACE v1 only.
- **BaceWiring extension to carry BaceAuthentication.** The
  `BaceAuthentication` type ships but is not plumbed into
  `BaceWiring`. Operators cannot select authenticated
  envelopes at open time until a carrier ADR lands.
- **CLI exposure.** `bradian-db` gains no new flags.
- **Key rotation, revocation, storage, derivation (KDF),
  transport, backup, audit.** None of these are addressed.
  Operators are responsible for key custody; this primitive
  only verifies "a party holding this exact key emitted this
  envelope."
- **Replay / rollback detection.** The MAC does not include
  any monotonic counter. A valid envelope can be copied
  across files or restored from an older backup and still
  verify.
- **Encryption / confidentiality.** Payload is plaintext
  (ADR 0066 ZSTD or RAW). This ADR adds authentication, not
  confidentiality.
- **Negotiation between a reader and a writer about which
  envelope to use.** Each carrier decides whether it emits
  BACE v1 or BACA. No mixed-envelope stream support at the
  carrier level.
- **Per-record MAC vs per-envelope MAC.** This primitive
  computes one MAC per envelope. Streaming authentication
  is out of scope.
- **Hardware security module (HSM) integration, key
  sealing, enclave support.**

## 3. Proposed design

### 3.1 BACA envelope v1 layout

| Offset | Size | Field                                   |
|-------:|-----:|-----------------------------------------|
|   0    |   4  | Magic `BACA`                            |
|   4    |   1  | Envelope version (`1`)                  |
|   5    |   1  | Reserved (must be `0`)                  |
|   6    |   2  | `codec_id` (u16 LE)                     |
|   8    |   2  | `codec_format_version` (u16 LE)         |
|  10    |   2  | `flags` (u16 LE)                        |
|  12    |   4  | `encoded_length` (u32 LE)               |
|  16    |   4  | `decoded_length` (u32 LE)               |
|  20    |  12  | Reserved for growth (must be all `0`)   |
|  32    |   N  | Encoded payload                         |
| 32+N   |   4  | CRC32 over header + payload             |
| 36+N   |  16  | BLAKE3-128 MAC tag over header + payload |

Total overhead: 52 bytes (vs BACE v1's 36). Rationale: 16-byte
truncated MAC provides 128-bit security against forgery under
the assumption that the key is secret. CRC32 is retained to
distinguish "payload truncation / disk corruption" from
"adversarial tampering with valid MAC" in error reporting.

### 3.2 MAC computation

```rust
use blake3::keyed_hash;

fn compute_mac(key: &[u8; 32], header: &[u8; 32], payload: &[u8]) -> [u8; 16] {
    let mut hasher = blake3::Hasher::new_keyed(key);
    hasher.update(header);
    hasher.update(payload);
    let mut tag = [0u8; 16];
    tag.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
    tag
}
```

Verify path uses `subtle::ConstantTimeEq`:

```rust
use subtle::ConstantTimeEq;

let expected = compute_mac(key, header, payload);
if expected.ct_eq(&observed).unwrap_u8() == 0 {
    return Err(AuthenticatedEnvelopeError::MacMismatch);
}
```

### 3.3 `BaceAuthentication` type

```rust
#[derive(Clone)]
pub struct BaceAuthentication {
    key: [u8; 32],
    label: String,          // operator-facing text, NOT in MAC input
}

impl BaceAuthentication {
    pub fn new(key: [u8; 32], label: impl Into<String>) -> Self { ... }
    pub fn label(&self) -> &str { &self.label }
    // intentionally no `pub fn key()` accessor — callers interact
    // via write/read helpers that take an &BaceAuthentication
}

impl std::fmt::Debug for BaceAuthentication {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BaceAuthentication {{ label: {:?}, key: [redacted] }}", self.label)
    }
}
```

The type deliberately does NOT expose the raw key bytes. A
test-only `#[cfg(test)]` getter may be provided if required
for golden-corpus tests, but production carriers only
pass `&BaceAuthentication` to the helpers in this module.

### 3.4 Error variants

```rust
#[derive(Debug, Error)]
pub enum AuthenticatedEnvelopeError {
    #[error("authenticated envelope magic mismatch (expected BACA)")]
    MagicMismatch,
    #[error("authenticated envelope version {declared} unsupported (expected 1)")]
    VersionMismatch { declared: u8 },
    #[error("authenticated envelope reserved bits non-zero")]
    ReservedBitsSet,
    #[error("authenticated envelope encoded_length {declared} exceeds cap {cap}")]
    EncodedLengthCapExceeded { declared: u32, cap: u32 },
    #[error("authenticated envelope decoded_length {declared} exceeds cap {cap}")]
    DecodedLengthCapExceeded { declared: u32, cap: u32 },
    #[error("authenticated envelope CRC32 mismatch")]
    CrcMismatch,
    #[error("authenticated envelope MAC mismatch (tag did not verify under provided key {label:?})")]
    MacMismatch { label: String },
    #[error("authenticated envelope buffer truncated")]
    Truncated,
}
```

CRC and MAC failures are distinct variants — the former
indicates damage, the latter indicates tampering or
key mismatch.

### 3.5 Compliance obligations (implementation test checklist)

1. `write_authenticated_envelope` + `read_authenticated_envelope`
   round-trip over a simple payload with a fixed key.
2. Byte-flip anywhere in the header fails `MacMismatch`.
3. Byte-flip anywhere in the payload fails `MacMismatch`.
4. Byte-flip only in the trailer CRC fails `CrcMismatch`
   BEFORE the MAC check runs (CRC is cheap; fail fast).
5. Byte-flip in the MAC tag fails `MacMismatch`.
6. Reading with a different 32-byte key fails `MacMismatch`.
7. Reading a legitimately-signed BACA envelope through the
   BACE v1 `read_envelope` fails `MagicMismatch` (so BACE and
   BACA streams don't accidentally cross-verify).
8. Writing a BACA envelope and then reading it back as a BACE
   envelope via the BACE v1 path also fails `MagicMismatch`.
9. Encoded/decoded length cap violations fail before MAC
   computation (matches ADR 0062 §3.3 bounded-decode rule).
10. `BaceAuthentication::Debug` does NOT print the key.
11. Constant-time comparison: a benchmark-based test is out
    of scope (timing attacks on local storage are a stretch
    threat), but the `subtle::ConstantTimeEq` usage is
    verified by a review-level assertion in the ADR + code
    comment at the call site.

## 4. Alternatives considered

**A. Encrypt payload instead of just authenticating it
(ChaCha20-Poly1305 AEAD).** Rejected for v1: confidentiality
is a separate threat model that drags in nonce management,
header-vs-body AAD plumbing, and backup/export semantics.
ChaCha20-Poly1305 is already in the workspace via
`dap-security`; a future ADR can add an encrypted variant
under a third envelope family (e.g., `BACZ` or `BACE+E`).

**B. HMAC-SHA256 instead of keyed BLAKE3.** Rejected: BLAKE3
is already in the workspace, is audited, and has a
purpose-built keyed mode that is simpler than HMAC
construction. Using an already-present primitive reduces
the dependency surface.

**C. Full 32-byte MAC tag instead of truncated 16-byte.**
Rejected: 128-bit security is adequate for storage
integrity; 16 bytes keeps the envelope compact. The
truncation strategy follows RFC 2104 §5 for truncated MACs
(not less than half the full length).

**D. Include a `key_id` field in the envelope header to
support rotation.** Rejected for v1: rotation needs a key
registry that's out of this ADR's scope. A future ADR can
add a 4-byte `key_id` field by growing the header's reserved
region (12 bytes available at offset 20).

**E. Combine CRC and MAC into one field.** Rejected: CRC
checks damage BEFORE the more expensive MAC runs, and
distinct error variants help operators diagnose "bit rot" vs
"wrong key" vs "tampering."

**F. Sign the envelope with an asymmetric signature (Ed25519)
instead of a symmetric MAC.** Rejected for v1: asymmetric
signatures have a different threat model (anyone with the
public key can verify) and pull in curve25519 crates. Future
ADR can add a `BACS` envelope family if that model is
needed.

## 5. Open design questions (owner review)

1. **Tag length.** Draft: 16 bytes (128-bit security).
   Alternatives: 32-byte full tag, 24-byte compromise.
   Confirm 16.
2. **MAC primitive.** Draft: BLAKE3 keyed hash via
   `blake3::keyed_hash`. Alternative: `hmac` crate with
   SHA-256 (adds `hmac` and `sha2` crate deps). Confirm
   BLAKE3.
3. **CRC preserved under BACA.** Draft: YES, for distinct
   damage-vs-tampering error classification. Alternative:
   omit CRC since MAC covers integrity. Confirm keep.
4. **`BaceAuthentication::Debug` redacts the key.** Draft:
   YES. Confirm.
5. **Test-only key accessor (`#[cfg(test)]`).** Draft:
   permitted if golden-corpus tests need it; otherwise the
   envelope round-trip tests pass the authentication to the
   write/read helpers and never touch the raw key. Confirm
   discretion.
6. **Carrier integration deferred.** Draft: this ADR adds
   only the primitive. BaceWiring is unchanged. Future ADR
   adds the wiring field and the carrier integration under
   its own threat-model section. Confirm defer.
7. **`subtle` crate dependency.** Draft: add as a direct
   `bradian-relational` dep (already a transitive dep via
   `chacha20poly1305`). Confirm.
8. **Reserved-bytes strict decode.** Draft: non-zero bytes
   in reserved[20..32] fail with `ReservedBitsSet`,
   matching the "zeros canonical" rule from ADRs 0062, 0067.
   Confirm.

## 6. Impact on invariants

- **INV-001 commit correctness:** N/A — no carrier integration.
- **INV-002 one authoritative state:** N/A.
- **INV-003 no authorization gap:** N/A. The primitive adds
  no authorization enforcement; future carrier ADRs are
  responsible for binding envelope authentication to the
  authorization model if that is claimed.
- **INV-004 no unsafe automatic availability:** N/A.
- **INV-005 recoverable lifecycle:** N/A for this primitive.
- **CLAUDE.md §6:** this ADR does NOT make a production
  security claim. It is a library primitive. A future
  carrier-integration ADR that claims "Bradian databases on
  disk are tamper-evident" will carry the §6 evidence
  requirements explicitly.

## 7. Dependencies this ADR would add

- `blake3 = { workspace = true }` — direct dep on
  `bradian-relational` (currently only a transitive dep via
  no `bradian-relational` path).
- `subtle = "2"` — direct dep on `bradian-relational`.
  Already a transitive dep via `chacha20poly1305`. Add to
  workspace `[workspace.dependencies]` or directly to
  `bradian-relational`.

No new error variants on any existing engine. No new policy
keys. No new CLI flags.

## 8. What this ADR explicitly does NOT do

- Touch any production carrier
  (`RowStore`, `MvccStore`, `DurableMvccStore`,
  `TypedMvccStore`, `DurableTypedStore`, the SQL engines,
  the backup carriers, the lifecycle coordinator, the
  analytical `dap-storage` crate).
- Extend `BaceWiring`.
- Add CLI exposure.
- Make a production security claim.
- Address replay, rotation, revocation, confidentiality,
  encryption, or any downstream security property.
- Change existing BACE envelope v1 behavior.
- Consume any carrier's existing on-disk bytes under BACA.

## 9. Acceptance evidence required before marking Accepted

1. ADRs 0060–0074 accepted — **satisfied**.
2. Owner sign-off on §5 open design questions, especially
   Q1 (tag length), Q2 (MAC primitive), Q6 (carrier
   integration deferral).
3. Implementation satisfying every item in §3.5.
4. Review-level inspection confirming:
   - the MAC primitive is `blake3::keyed_hash` (no custom
     crypto per CLAUDE.md §3).
   - MAC comparison uses `subtle::ConstantTimeEq` with an
     inline comment at the call site referencing this ADR.
   - `BaceAuthentication::Debug` does not print key bytes
     (manual tests of the format string are included).
5. No regressions in any existing `bradian-relational` or
   `dap-admin` test.

Only then can this ADR move from **PROPOSED** to **ACCEPTED**.
This ADR landing does NOT constitute shipping a security
feature; the feature ships when a later carrier-integration
ADR consumes this primitive.
