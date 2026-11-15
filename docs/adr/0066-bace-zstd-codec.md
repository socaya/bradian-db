# ADR 0066 — BACE ZSTD codec (implementation; **ACCEPTED**)

**Status: ACCEPTED, 14 November 2026.** Owner accepted with the
drafted defaults; implementation landed in the same commit:

- `crates/bradian-relational/src/bace/codec.rs` — `zstd::ZstdCodec`
  at `CodecId(49)`, `format_version = 1`.
- `crates/bradian-relational/src/bace/registry.rs` —
  `BuiltInRegistry::new()` now registers RAW + ZSTD.
- `crates/bradian-relational/Cargo.toml` — direct `zstd = { workspace = true }`
  dep added (was already a transitive dep via `bradian-storage`).
- `crates/bradian-relational/tests/row_store_bace.rs` — 4 new
  integration tests; `src/bace/codec.rs` carries 6 new unit tests
  covering the §3.6 golden corpus and §3.4 bounded-decode paths.

940 workspace tests pass; clippy clean on
`bradian-relational --all-targets`.

### Honesty note on §3.8 item 5 (revised in flight)

The drafted §3.8 test item 5 claimed the ZSTD-wrapped snapshot file
would be strictly smaller than the Raw baseline. In practice BRPG
pages are fixed 4 KiB on disk: a 4028-byte plain payload that ZSTD
compresses to ~30 bytes still occupies a 4096-byte page. Per-page
compression at a fixed physical page size does not shrink the
snapshot file on its own. The test actually shipped —
`zstd_balanced_profile_envelope_encoded_length_is_smaller_than_decoded_on_redundant_data`
— asserts the honest, observable claim: at the envelope layer,
every v2 data page emitted under ZSTD carries
`encoded_length < decoded_length` on redundant input (in fact
`encoded_length < decoded_length / 10`). Physical snapshot-file
savings require variable-size pages or multi-logical-page
coalescing; that work is a separate future ADR.

## 1. Context

- ADR 0061 §3.1 reserved codec IDs 48–63 for outer stream
  compression. RAW took ID 48 in Slice Q. ZSTD is the natural
  next ID.
- ADR 0061 §5 item 3 (resolved) planned ZSTD as the second codec
  after RAW's determinism corpus proves out.
- ADR 0061 §5 item 4 (resolved): ZSTD is `HardwareDependent` for
  the first implementation until a pinned library version + SIMD
  path produces a reproducible golden corpus on every supported
  target.
- ADR 0061 §9 item 7 (resolved): format-version strategy is
  monotone `u16` per codec. ZSTD starts at `format_version = 1`.
- BACE §6 `.derc` already uses ZSTD level 3 as the default wrap,
  independent of the BACE policy service. That hardcode is left
  in place per ADR 0060 §5 item 4; the BACE ZSTD codec is a
  separate, policy-governed integration (not a reuse of the
  `.derc` layer).
- Workspace already pins `zstd = "0.13"` for `bradian-storage`; this
  ADR adds it as a direct dep on `bradian-relational`.

## 2. Scope of this ADR

**In scope:**

- A new `bace::codec::zstd::ZstdCodec` at `CodecId(49)` with
  `format_version = 1`.
- Registration via `BuiltInRegistry::new()` so every build ships
  ZSTD alongside RAW.
- Default compression level pinned to **ZSTD level 3** (matches
  the `.derc` hardcode; `ZSTD_CLEVEL_DEFAULT`).
- Capability descriptor: `Determinism::HardwareDependent`,
  `flags.critical = false`, `flags.allows_outer_compression =
  false` (ZSTD is itself an outer compressor).
- Bounded decode per ADR 0062 §3.3 and BACE-FMT-003: the decoder
  honours `decoded_length` as a hard upper bound during decoding,
  not merely after.
- A golden-corpus commitment per ADR 0061 §3.7: fixed `(input,
  level, expected_output)` tuples that CI runs on every supported
  target. For this first implementation the corpus is kept in the
  test file; a sibling ADR can promote it to an external artifact
  later.
- Integration test demonstrating that under `CompressionProfile::
  Balanced` with `allowed_codecs = [ZSTD_ID]`, a repetitive
  payload produces a **smaller snapshot** than the equivalent
  `CompressionProfile::Raw` baseline.

**Explicitly out of scope** (each needs its own ADR or follow-up):

- **Changing the default `PlatformBaseline::allowed_codecs`** to
  include ZSTD. The default stays `[RAW_ID]`; operators opt in to
  ZSTD by setting `allowed_codecs = [ZSTD_ID, RAW_ID]` on the
  wiring they pass to `open_with_bace`. This preserves the "no
  silent format change" culture for existing BACE opt-in stores.
- **A `compression.zstd_level` policy key.** Level 3 is hardcoded
  for the first implementation; a follow-up ADR adds a per-codec
  parameter via the ADR 0061 §3.3 `parameters` field (currently
  not exposed in Slice Q `CodecCapability`).
- **LZ4 or other outer codecs.** Separate ADRs.
- **Dictionaries** (BACE §4.2 BACE-FR-COL-002). ZSTD supports
  dictionary mode; this ADR does not use it. Dictionary scope
  and lifecycle is ADR 0070 (not drafted).
- **BACE-SEL-002 avoid-expansion selection logic.** With ZSTD
  live, this becomes useful; a follow-up ADR implements the
  per-page `if encoded_bytes >= plain_bytes * (100 - min_saving_percent)
  / 100 then fall back to RAW envelope` rule. Until then, ZSTD
  encodes unconditionally; repetitive data saves space;
  incompressible data costs 36 bytes of envelope overhead.
- **Streaming encode/decode for payloads larger than a page.**
  The envelope is bounded to one 4 KiB page; streaming does not
  apply.
- **Authenticated variant** (`"BACA"` magic reserved by ADR 0062).
  ZSTD under the v1 envelope is CRC-only, host-local trust.

## 3. Proposed design

### 3.1 Codec identity

```rust
pub const ZSTD_ID: CodecId = CodecId(49);
pub const ZSTD_FORMAT_VERSION: u16 = 1;
pub const ZSTD_LEVEL: i32 = 3;  // matches .derc default, ZSTD_CLEVEL_DEFAULT
```

The codec sits at `CodecId(49)` inside the outer-compression range
48–63 (ADR 0061 §3.1). No inner-encoding ID (16–47) is allocated
by this ADR; P1 remains deferred.

### 3.2 `CodecCapability` descriptor

```rust
CodecCapability {
    id: ZSTD_ID,
    format_version: ZSTD_FORMAT_VERSION,
    max_input_bytes: u32::MAX,
    max_output_bytes: u32::MAX,
    determinism: Determinism::HardwareDependent,
    flags: CapabilityFlags {
        critical: false,                 // RAW stays the fallback
        experimental: false,
        preserves_null_distinct: true,
        allows_outer_compression: false, // ZSTD IS outer
    },
    name: "ZSTD",
}
```

`HardwareDependent` is deliberate. ZSTD's reference implementation
produces bit-identical output for a given `(level, version, input)`
tuple **in principle**, but SIMD paths, platform-specific
configuration flags and version-to-version behaviour drift mean we
cannot claim `Deterministic` without a per-target golden corpus
rolling check. The golden corpus in §3.6 pins the test inputs; any
deviation in CI is a release-blocking defect regardless of class.

### 3.3 Encode

```rust
fn encode(&self, input: &[u8], output: &mut Vec<u8>) -> Result<usize, EncodeError> {
    // Compress at a pinned level. zstd::bulk::compress uses the
    // reference ZSTD frame format (not the raw block format), so
    // the output is self-describing and decodable without external
    // framing metadata.
    let compressed = zstd::bulk::compress(input, ZSTD_LEVEL)
        .map_err(|_| EncodeError::OutputTooLarge { bytes: input.len(), limit: u32::MAX })?;
    if compressed.len() > u32::MAX as usize {
        return Err(EncodeError::OutputTooLarge { bytes: compressed.len(), limit: u32::MAX });
    }
    output.extend_from_slice(&compressed);
    Ok(compressed.len())
}
```

Rationale for frame format over raw block:

- Frame format is self-describing (magic, version, content size).
- Decoding can bound output via the frame's declared content size.
- Compatible with `zstd` CLI and every ZSTD-aware reader if we
  ever need to inspect the payload out-of-band.

### 3.4 Decode (bounded output)

```rust
fn decode(&self, input: &[u8], decoded_length: u32, output: &mut Vec<u8>) -> Result<(), DecodeError> {
    // zstd::bulk::decompress takes an explicit output capacity. We
    // pass decoded_length as that capacity so the ZSTD library
    // itself enforces the bound DURING decoding — matching
    // BACE-FMT-003 "enforce bounded output during decoding, not
    // merely after decode completion."
    let result = zstd::bulk::decompress(input, decoded_length as usize)
        .map_err(|_| DecodeError::DecodedLengthExceeded {
            bytes: usize::MAX, declared: decoded_length,
        })?;
    if result.len() as u64 != decoded_length as u64 {
        return Err(DecodeError::DecodedLengthMismatch {
            bytes: result.len(), declared: decoded_length,
        });
    }
    output.extend_from_slice(&result);
    Ok(())
}
```

Critical invariant: `decoded_length` is the **ADR 0062 envelope-
declared** decoded length, not a policy cap. The envelope reader
in `bace::envelope::read_envelope` has already enforced the policy
cap on `decoded_length` before this function is called.

### 3.5 Registry hook

```rust
// bace::registry::BuiltInRegistry::new()
pub fn new() -> Self {
    Self::with_codecs(vec![
        Box::new(RawCodec),
        Box::new(ZstdCodec),   // new in ADR 0066
    ])
}
```

Both codecs ship in every build. The capability-negotiation
algorithm (ADR 0061 §3.5) remains unchanged; a remote peer that
does not know ZSTD will see it marked `critical: false` and treat
it as optional.

### 3.6 Golden corpus (ADR 0061 §3.7, BACE-NFR-PORT-001)

Three fixed `(label, input, expected_length_range)` tuples that
every target must satisfy on every build:

| Label | Input | Expected `output.len()` constraint |
|---|---|---|
| `zstd_empty_input` | `b""` | ≥ 9 bytes (ZSTD frame overhead) and round-trips to empty |
| `zstd_short_incompressible` | `b"abcdef"` | round-trips byte-exactly |
| `zstd_long_repetitive` | 4 000 copies of `b"x"` | `< 100` bytes AND round-trips byte-exactly |

The "incompressible" case verifies round-trip when ZSTD cannot
compress. The "repetitive" case verifies that compression **does
save space** (BACE-GOAL-001 minimum practical physical footprint).

Byte-identical output across targets is not required (HardwareDependent),
only round-trip correctness and the declared-size-range bounds.

### 3.7 Row-store integration — operator opt-in

A store opt-in to ZSTD at a specific carrier:

```rust
let wiring = BaceWiring {
    policy_service: Arc::new(InMemoryPolicyService::new(
        PlatformBaseline {
            profile: CompressionProfile::Balanced,
            // Order matters: pick_codec picks the first registered ID.
            allowed_codecs: vec![ZSTD_ID, RAW_ID],
            ..PlatformBaseline::shipped()
        },
        BuiltInRegistry::new(),
    )),
    registry: Arc::new(BuiltInRegistry::new()),
};
let store = RowStore::open_with_bace(root, StorageLimits::default(), wiring)?;
```

The ADR 0065 §3.2 `pick_codec` picks the first codec from the
allow-set whose capability is registered. Placing `ZSTD_ID` first
in the slice causes the writer to emit ZSTD-wrapped v2 pages.

**No change to the default `PlatformBaseline::shipped()`.** The
default `allowed_codecs` remains `[RAW_ID]`. Existing BACE
opt-in stores do not silently switch codecs; operators who want
space savings pass `[ZSTD_ID, RAW_ID]` explicitly.

### 3.8 Compliance obligations (implementation test checklist)

An implementation of this ADR MUST demonstrate every item below:

1. `BuiltInRegistry::new()` registers ZSTD at ID 49.
2. ZSTD capability descriptor matches §3.2.
3. Round-trip of empty, short-incompressible, and long-repetitive
   golden-corpus inputs via the `Codec` trait.
4. Round-trip through the BACE envelope (compress → wrap →
   `write_envelope` → `read_envelope` → `decode` → compare).
5. `balanced_profile_with_zstd_saves_space_on_repetitive_data` —
   a `RowStore` opened with `open_with_bace`,
   `allowed_codecs=[ZSTD_ID, RAW_ID]`, 4 000 copies of a tiny
   repeating row, checkpointed under Balanced yields a snapshot
   file **smaller** than the Raw-profile baseline on the same rows.
6. Fail-closed on a declared-length-mismatch envelope: an
   envelope whose `decoded_length` field disagrees with the real
   decoded size fails closed with
   `DecodeError::DecodedLengthMismatch`.
7. Existing `tests/row_store_bace.rs` tests all still pass (RAW
   remains critical; the new ZSTD codec coexists but does not
   disturb RAW-only policies).

## 4. Alternatives considered

**A. Use `zstd`'s raw block format instead of the frame format.**
Smaller per-payload overhead (no frame header). Rejected: raw
blocks don't carry content size, making bounded decode harder,
and we lose interop with any ZSTD-aware external reader.

**B. Register ZSTD in `PlatformBaseline::allowed_codecs` by
default.** Space savings out of the box. Rejected: silently
changes on-disk format for existing BACE opt-in stores. The
project culture is "explicit opt-in to new formats"; operators
add `ZSTD_ID` to the allow-set themselves.

**C. Mark ZSTD `Deterministic` via a pinned version lock in
Cargo.toml.** Simpler production obligation. Rejected: even
pinned, ZSTD's `zstd-sys` C library uses SIMD intrinsics that can
produce different byte output across x86/ARM/aarch64 at the same
level. The HardwareDependent class is the honest contract for the
first implementation.

**D. Expose `compression.zstd_level` as a policy key now.**
Operator flexibility. Rejected: the Slice Q `CodecCapability`
does not expose a `parameters` schema yet. Adding a per-codec
parameter needs an ADR 0061 amendment first; the current ADR
hardcodes level 3 (matches `.derc`).

**E. Reuse `bradian-storage`'s existing `.derc` ZSTD wrap via a
shared helper crate.** Code reuse. Rejected: `.derc` uses ZSTD
without the BACE envelope. Collapsing the two would couple the
BACE lifecycle to `.derc`'s v1 format and vice versa. Each
subsystem maintains its own compression wrapper.

## 5. Open design questions (owner review)

1. **ZSTD compression level.** Draft pins level 3. Alternatives:
   level 1 (fast), level 10, level 19 (max). Level 3 matches
   `.derc` and is the ZSTD default; confirm.
2. **Codec ID 49 vs a later number in 48–63.** Draft picks 49
   immediately after RAW. Alternatives: 50 to leave 49 reserved
   for a future codec family between RAW and ZSTD. Picking 49 is
   the natural neighbour; confirm.
3. **`HardwareDependent` vs `Deterministic` with a locked ZSTD
   version.** Draft picks `HardwareDependent`. Changing later
   requires the release-blocking golden-corpus work. Confirm the
   conservative stance.
4. **Allow-set ordering** — does the policy service treat
   `allowed_codecs` as an **ordered preference list** or an
   **unordered set**? Draft: ordered (first-match wins), matching
   the Slice R `pick_codec` behaviour. Confirm.
5. **BACE-SEL-002 avoid-expansion.** Draft defers to a later ADR.
   Alternative: implement the simple comparison now so
   incompressible pages fall back to RAW envelope instead of
   paying 36 bytes of ZSTD overhead on a payload that didn't
   shrink. Confirm defer.
6. **Default `PlatformBaseline::allowed_codecs`.** Draft stays
   `[RAW_ID]`. Alternative: flip to `[ZSTD_ID, RAW_ID]` so new
   `open_with_bace` stores get space savings by default. Confirm
   the conservative stance.
7. **Golden corpus permanence.** Draft keeps the three fixed
   tuples in the test file. Alternative: external artifact under
   `fixtures/bace/zstd/` for cross-tool consumption. Confirm.

## 6. Impact on invariants

- **INV-001 commit correctness:** no change. Encoded bytes differ
  from plain bytes but commit semantics are unchanged.
- **INV-002 one authoritative state:** no change. Row state
  stays authoritative.
- **INV-003 no authorization gap:** no change. ZSTD encodes
  bytes already within an authorized scope (checkpoint construction
  runs under the mutation mutex).
- **INV-004 no unsafe automatic availability:** N/A.
- **INV-005 recoverable lifecycle:** ZSTD's `format_version = 1`
  starts the per-codec rollback story. A future ZSTD bump (new
  library major version with breaking byte-format change) must
  arrive as `format_version = 2` with explicit migration per
  BACE-FMT-004.

## 7. Dependencies this ADR would add

`zstd = "0.13"` as a **direct** dep on `bradian-relational`
(currently only in `bradian-storage` as a direct dep; already in the
workspace pin and Cargo.lock graph). The crate is in the CLAUDE.md
§5 allowlist under "Compression" (`zstd`, `lz4_flex`). No new
transitive dep.

## 8. What this ADR explicitly does NOT do

- Modify `row_store.rs`, `page.rs`, or the `bace::envelope` /
  `bace::policy` modules. Integration happens by **registering**
  ZSTD in `BuiltInRegistry`; the carrier plumbing from Slices Q
  and R is unchanged.
- Change the default `PlatformBaseline::allowed_codecs`.
- Expose `compression.zstd_level` as a policy key.
- Implement BACE-SEL-002 avoid-expansion.
- Use ZSTD dictionaries.
- Touch `.derc` or `BRWL`.

## 9. Acceptance evidence required before marking Accepted

1. ADRs 0060–0065 accepted — **satisfied**.
2. Owner sign-off on §5 open design questions, especially Q1
   (level), Q5 (defer avoid-expansion), Q6 (default allow-set
   unchanged).
3. Threat-model review confirming `HardwareDependent` codec adoption
   does not weaken the BACE-NFR-PORT-001 obligation (ADR 0061 §9
   item 2 was deferred for Slice Q RAW-only; this codec triggers
   it).
4. Golden corpus committed alongside the implementation.
5. Explicit statement that no production BACE-opt-in store
   silently switches to ZSTD: the default `PlatformBaseline`
   remains `[RAW_ID]` and operators opt in by passing a wiring
   with `[ZSTD_ID, RAW_ID]`.

Only then can this ADR move from **PROPOSED** to **ACCEPTED**, and
only then can `bace::codec::zstd::ZstdCodec` ship in the shipped
`BuiltInRegistry::new()`.
