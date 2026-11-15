# BACE authenticated envelope primitive verification (ADR 0075)

15 November 2026. Baseline: `74e882e`. Adds a new envelope
family under the `BACA` magic with a BLAKE3-128 keyed MAC.

**This ADR does NOT make a production security claim** per
CLAUDE.md §6. It ships a library primitive. A future carrier-
integration ADR that claims "Bradian is tamper-evident on
disk" will carry the §6 evidence requirements (isolation,
revocation, audit, rotation, backup protection, incident
response).

## Executed result

**995 Rust tests passed, 0 failed, 0 ignored** (982 → 995).
Clippy clean on `bradian-relational --all-targets`.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-relational` | Passed |
| `cargo test --workspace --locked` | 995 passed, 0 failed |
| `cargo test -p bradian-relational --lib authenticated` | 13 passed |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## What landed

### Workspace `Cargo.toml`

```toml
subtle = "2"           # new pin
```

### `crates/bradian-relational/Cargo.toml`

```toml
blake3 = { workspace = true }   # promoted from no-dep to direct
subtle = { workspace = true }   # new
```

### `crates/bradian-relational/src/bace/authenticated.rs` (new)

Public API surface:

```rust
pub const AUTHENTICATED_ENVELOPE_MAGIC: [u8; 4] = *b"BACA";
pub const AUTHENTICATED_ENVELOPE_VERSION: u16 = 1;
pub const AUTHENTICATED_ENVELOPE_HEADER_SIZE: usize = 32;
pub const AUTHENTICATED_ENVELOPE_TRAILER_SIZE: usize = 4;
pub const AUTHENTICATED_ENVELOPE_MAC_SIZE: usize = 16;
pub const AUTHENTICATED_ENVELOPE_MIN_SIZE: usize = 52;

pub struct AuthenticatedEnvelopeFlags { ... }
pub struct AuthenticatedEnvelopeHeader { ... }
pub struct AuthenticatedEnvelope<'a> { ... }
pub struct AuthenticatedEnvelopeCaps { ... }

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AuthenticatedEnvelopeError { ... }   // 11 variants

pub struct BaceAuthentication { ... }         // Debug redacts key

pub fn write_authenticated_envelope(
    output: &mut Vec<u8>,
    header: &AuthenticatedEnvelopeHeader,
    payload: &[u8],
    auth: &BaceAuthentication,
) -> Result<usize, AuthenticatedEnvelopeError>;

pub fn read_authenticated_envelope<'a>(
    input: &'a [u8],
    caps: AuthenticatedEnvelopeCaps,
    auth: &BaceAuthentication,
) -> Result<AuthenticatedEnvelope<'a>, AuthenticatedEnvelopeError>;
```

Key implementation points:

- **MAC primitive:** `blake3::Hasher::new_keyed(key)` over
  header bytes + payload bytes, truncated to 16 bytes
  (BLAKE3-128). No custom crypto per CLAUDE.md §3.
- **Constant-time comparison:** `subtle::ConstantTimeEq` on
  the tag. Inline comment at the call site references ADR
  0075 §3.2 so the primitive is identifiable by eye.
- **Step order:** min-size → magic → version → reserved bits
  → length caps (BEFORE allocation) → body-length match →
  CRC (fast fail for bit rot) → MAC (constant-time).
- **`BaceAuthentication::Debug`:** redacted; prints only the
  label. `PartialEq` uses constant-time key equality.
- **No raw-key accessor** in the public API. A
  `pub(crate) key_bytes()` is used internally by the write
  and read helpers; nothing outside the crate can extract
  the key bytes.

### `crates/bradian-relational/src/bace/mod.rs`

```rust
pub mod authenticated;
pub use authenticated::{
    read_authenticated_envelope, write_authenticated_envelope,
    AuthenticatedEnvelope, AuthenticatedEnvelopeCaps,
    AuthenticatedEnvelopeError, AuthenticatedEnvelopeFlags,
    AuthenticatedEnvelopeHeader, BaceAuthentication,
    AUTHENTICATED_ENVELOPE_HEADER_SIZE, AUTHENTICATED_ENVELOPE_MAC_SIZE,
    AUTHENTICATED_ENVELOPE_MAGIC, AUTHENTICATED_ENVELOPE_MIN_SIZE,
    AUTHENTICATED_ENVELOPE_TRAILER_SIZE, AUTHENTICATED_ENVELOPE_VERSION,
};
```

### 13 unit tests

| # | Test | ADR §3.5 item |
|---|---|---|
| 1 | `write_then_read_round_trips_payload_under_fixed_key` | 1 |
| 2 | `empty_payload_round_trips` | n/a — minimum-size |
| 3 | `header_byte_flip_fails_mac_mismatch` | 2 (MAC or CRC acceptable) |
| 4 | `payload_byte_flip_fails_mac_mismatch` | 3 (CRC fires first on body damage) |
| 5 | `trailer_crc_byte_flip_fails_crc_before_mac` | 4 |
| 6 | `mac_byte_flip_fails_mac_mismatch_with_crc_intact` | 5 |
| 7 | `reading_with_different_key_fails_mac_mismatch` | 6 |
| 8 | `baca_envelope_fails_as_bace_v1` | 7 |
| 9 | `bace_v1_envelope_fails_as_baca` | 8 |
| 10 | `decoded_length_cap_fires_before_mac_computation` | 9 |
| 11 | `bace_authentication_debug_does_not_print_key_bytes` | 10 |
| 12 | `mac_verification_uses_subtle_constant_time_comparison` | 11 |
| 13 | `truncated_envelope_fails_truncated_or_input_too_small` | n/a — truncation shape |

### §3.5 outcome adjustments documented in ADR

- Item 2 (header flip): CRC input also moves when header
  changes, so CRC can fire before MAC. Test accepts either
  variant; invariant is "read fails."
- Item 3 (payload flip): CRC covers payload and fires first.
  Test asserts `CrcMismatch` matching the §3.4 step order.
- All other items match drafted outcomes.

## Compliance against ADR 0075 §9 acceptance evidence

1. ADRs 0060–0074 accepted — ✓
2. §5 open design questions: landed per drafted defaults.
3. Items 1–13 of §3.5 — ✓ (with honest outcome adjustments
   above).
4. Review-level inspection:
   - MAC primitive is `blake3::Hasher::new_keyed` — ✓ visible
     in `compute_mac` function.
   - MAC comparison uses `subtle::ConstantTimeEq` with an
     inline comment referencing ADR 0075 §3.2 — ✓ at
     `read_authenticated_envelope` step 8.
   - `BaceAuthentication::Debug` does not print key bytes
     — ✓ asserted by test 11, impl uses `.field("key",
     &"[redacted]")`.
5. No regressions — ✓ (995 pass, 0 fail).

## What this ADR explicitly does NOT do (unchanged)

- Touch any production carrier
  (`RowStore`, `MvccStore`, `DurableMvccStore`,
  `TypedMvccStore`, `DurableTypedStore`, the SQL engines,
  the backup carriers, the lifecycle coordinator, the
  analytical `bradian-storage` crate).
- Extend `BaceWiring`.
- Add CLI exposure.
- Make a production security claim.
- Address replay, rotation, revocation, confidentiality,
  encryption, or any downstream security property.

## BACE workstream progress (as of this ADR)

| ADR | Component | Status |
|---|---|---|
| 0060–0066 | Prerequisites + RowStore + ZSTD | ACCEPTED |
| 0067–0068 | Backup carrier adapters | ACCEPTED |
| 0069–0072 | All four SQL engines BACE open | ACCEPTED |
| 0073 | `bradian-db` CLI BACE flags | ACCEPTED |
| 0074 | G23 DDL demonstration tests | ACCEPTED |
| **0075** | **BACE authenticated envelope primitive (BACA)** | **ACCEPTED** |

## Remaining BACE workstream candidates

- **`.derc` carrier BACE** — analytical column segments in
  `bradian-storage` crate; format version bump; cross-crate.
- **BRWL carrier BACE** — WAL frame compression on
  latency-sensitive write path; benchmarking required.
- **Dictionary scope** — BACE §4.2 dictionary mode with
  per-column lifecycle.
- **Carrier integration of `BaceAuthentication`** — the
  concrete security feature that would consume this ADR's
  primitive; needs the CLAUDE.md §6 evidence package.

## Baseline + commit

Baseline: `d11a9b2` (ADR 0075 PROPOSED).
Implementation commit: assigned by this commit.

See [ADR 0075](../adr/0075-authenticated-envelope.md).
