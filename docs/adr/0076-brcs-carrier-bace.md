# ADR 0076 — `.brcs` carrier BACE adapter (implementation; **ACCEPTED**)

**Status: ACCEPTED, 15 November 2026.** Owner accepted with the
drafted defaults; implementation landed in the same commit as
the R.1–R.4 sub-slices from §3.6.

First ADR in the BACE series that touches `bradian-storage`. `.brcs`
format minor-version bump v1.0 → v1.1. New
`FeatureFlags::BACE_PAGE_ENVELOPE` plus per-page trailer byte
infrastructure. Legacy v1.0 segments stay byte-identical; v1.0
readers reject v1.1 cleanly via the feature-flag check; v1.1
readers accept both.

Implementation summary:

- `crates/bradian-storage/Cargo.toml` — direct dep on
  `bradian-relational` (first cross-crate BACE dep).
- `crates/bradian-storage/src/segment/format.rs` — new
  `SEGMENT_VERSION_MINOR_V1_1 = 1`, new
  `FeatureFlags::BACE_PAGE_ENVELOPE = 1 << 5`,
  `ALL_SUPPORTED_V1_1` superset, `is_subset_supported_v1_1`
  helper, new `BaceEnvelopeTag` enum.
- `crates/bradian-storage/src/segment/writer.rs` —
  `SegmentWriter::new_with_bace`, `bace_wiring` field, writer
  emits BACE v1 envelope (via
  `bradian_relational::bace::envelope::write_envelope`) when
  `bace_active`, plus per-page trailer byte.
- `crates/bradian-storage/src/segment/reader.rs` —
  `SegmentReader::open_with_bace`, `_bace_feature_active` flag,
  reader reads trailer byte and dispatches on
  `BaceEnvelopeTag::{None, BaceV1, BacaV1Reserved}` with clean
  errors for unknown/reserved values.
- `crates/bradian-storage/tests/brcs_bace.rs` — 10 new integration
  tests covering §3.5 compliance.

1005 workspace tests pass (995 → 1005); clippy clean on
`cargo clippy --workspace --all-targets --locked -- -D warnings`.

## 1. Context

- `.brcs` v1 is the Bradian analytical columnar segment format
  (one file per segment under
  `<root>/datasets/<id>/partitions/<id>/segments/<id>.brcs`).
  Current layout: `SEGMENT_MAGIC "BRCS\x01SEG"`, u16 major
  version `1`, u16 minor `0`, 20-byte header reserved, N
  compressed pages, bincode-serialized `SegmentFooter`,
  `FOOTER_MAGIC`.
- Each page's body is one of:
  - `compressed=false`: inner codec output verbatim
    (`CodecId::Plain`, `PlainUtf8`, `Dictionary`, `RunLength`,
    `Delta`, `BitPacked`)
  - `compressed=true`: a raw `zstd::bulk::compress` frame of
    the inner codec output
- `FeatureFlags::ALL_SUPPORTED_V1` = `COMPRESSED_PAGES |
  DICT_ENCODING | PAGE_CRC32`. Reserved bits are rejected by
  the reader.
- This path is entirely separate from `bradian-relational`'s
  BRPG v2 pages (ADR 0064). The two formats do not share
  bytes, codecs, or recovery paths.
- Bringing `.brcs` under BACE buys:
  - **Uniform codec governance** — operators control
    compression through one `BaceWiring` instead of the
    scattered `options.compress: bool` on the writer.
  - **Bounded decode** — `zstd::bulk::decompress` in the
    current reader accepts an output capacity equal to
    `uncompressed_len`, but there is no caller-supplied cap
    distinct from the on-disk declaration. BACE adds one.
  - **Codec extensibility** — future outer codecs (LZ4 etc.)
    plug in via the BACE registry without `.brcs` format
    changes.
  - **Authenticated variant readiness** — once a carrier-
    integration ADR consumes ADR 0075's `BACA` envelope,
    `.brcs` segments inherit tamper-evidence for free.

## 2. Scope of this ADR

**In scope:**

- `.brcs` format v2: minor version bump to
  `SEGMENT_VERSION_MAJOR = 1`, `SEGMENT_VERSION_MINOR = 1`.
  Readers that know v1.1 can read both v1.0 and v1.1
  segments. v1.0-only readers REJECT v1.1 segments through a
  new feature flag (see §3.2).
- New `FeatureFlags::BACE_PAGE_ENVELOPE = 1 << 5`. A v1.1
  segment that uses BACE envelopes sets this bit; a v1.1
  segment that doesn't (legacy write path still supported)
  leaves it clear. v1.0 readers reject segments with the bit
  set because `is_subset_supported` fails.
- New `PageMeta::bace_envelope` field, `u8`, defaults to `0`
  (legacy per-page ZSTD via `compressed=true` or plain via
  `compressed=false`). Non-zero values indicate BACE
  envelopes:
  - `1` — BACE v1 (`BACE` magic from ADR 0062)
  - `2` — reserved for BACA (ADR 0075); not emitted by this
    ADR.
- **Cross-crate dep:** `bradian-storage/Cargo.toml` gains a direct
  dep on `bradian-relational` (currently has no such
  dependency). This is a significant architectural coupling
  and is called out in §5 Q2 for explicit owner review.
- Writer: `SegmentWriter::new_with_bace(file, options, wiring)`
  that threads a `BaceWiring` through page encoding. When
  `wiring.is_some()` AND the resolved codec is non-RAW, each
  page's body is wrapped in a BACE v1 envelope whose
  `codec_id` matches the resolved BACE codec (ZSTD=49, RAW=48).
  Legacy `SegmentWriter::new` is unchanged.
- Reader: `SegmentReader::open_with_bace(path, wiring)`.
  When the segment's `bace_envelope` field on a page is
  non-zero, the reader routes through BACE envelope decode.
  Legacy pages (`bace_envelope=0`) go through the existing
  `zstd::bulk::decompress` path unchanged.
- Policy-caps integration: the reader uses the same
  `compression.max_decoded_page_bytes` from the policy
  service, resolved at `ScopeChain::baseline_only()`
  consistent with ADR 0065.
- Tests covering §3.5 checklist.

**Explicitly out of scope** (each a future ADR or follow-up):

- **Authenticated `.brcs` segments (BACA).** The format
  reserves `bace_envelope=2`, but no production writer emits
  it. A future carrier-integration ADR for BACA across
  carriers lands that path.
- **Changes to the inner codec set** (`CodecId::Plain`,
  `Dictionary`, etc.). BACE is an outer wrapper around
  whatever the inner codec produces.
- **Dictionary-scope integration** (BACE §4.2). Separate ADR.
- **CLI exposure on `bradian-admin` operator commands that touch
  `.brcs`** (`ExportSnapshot`, `RestoreSnapshot`,
  `CompactPartition`). Follow-up ADR extends the CLI; this
  ADR adds only library APIs.
- **Encryption of `.brcs` payload** (ChaCha20-Poly1305 or
  similar). Separate ADR.
- **Format-level changes beyond the two additions** (new
  feature flag, new PageMeta field).
- **Migration from v1.0 to v1.1 in-place.** v1.0 segments
  stay v1.0 forever; compaction / rewrite is the only path
  to v1.1.
- **Rename `.brcs` → `.bradian-column` or any `dap-*` crate
  rename.** Per CLAUDE.md §4, `.brcs` and `dap-*` are
  retained internal identifiers.

## 3. Proposed design

### 3.1 On-disk format v1.1

**Segment file layout** — unchanged byte sequence up to and
including the per-page bodies. Only `PageMeta` (inside the
bincode-serialized `SegmentFooter`) grows a field:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PageMeta {
    pub codec: CodecId,
    pub compressed: bool,
    pub payload_offset: u64,
    pub payload_len: u32,
    pub uncompressed_len: u32,
    pub validity_offset: u64,
    pub validity_len: u32,
    pub row_count: u32,
    pub zone_map: ZoneMap,
    pub crc32: u32,
    /// ADR 0076 — v1.1. 0 = legacy (compressed field drives
    /// the body format). 1 = BACE v1 envelope. 2 = reserved
    /// for BACA (ADR 0075); not produced by this ADR.
    #[serde(default)]
    pub bace_envelope: u8,
}
```

**Bincode compatibility caveat.** Standard `bincode` is
strict about field counts; adding a field to a struct
deserialized through `bincode::deserialize` breaks on v1.0
footers unless the serializer is configured to tolerate
trailing bytes AND the reader is configured accordingly.

The current reader uses
`bincode::deserialize::<SegmentFooter>(&payload)`. To
preserve v1.0 compatibility, the implementation MUST either:

- Option A: **Bump the segment major version to 2** and
  **drop v1.0 compatibility** in the new reader (clean break;
  rewrite required for existing data).
- Option B: Keep the major version at 1, handle the v1.0
  footer through a custom deserializer that applies
  `#[serde(default)]` semantics (bincode does not do this
  by default).
- Option C: Store the new `bace_envelope` byte **outside**
  the bincode-serialized footer — e.g., as a per-page trailer
  byte immediately after each page's validity bitmap. This
  avoids the bincode struct-field-count problem entirely.

**Draft chooses Option C** because it preserves v1.0
read/write without rewrite and keeps the format change
minimal. Specifically:

- v1.1 writers append one extra byte `bace_envelope: u8`
  after each page's validity bitmap (or after the payload
  when validity is empty).
- The `FeatureFlags::BACE_PAGE_ENVELOPE` bit in the segment
  header tells the reader whether this trailer byte is
  present. v1.0 segments have the bit clear and no trailer
  byte; v1.1 segments set the bit and include the byte.
- `PageMeta.payload_len` continues to count the compressed-
  payload length only; it does NOT include the new trailer
  byte.
- The crc32 field continues to cover (payload || validity)
  only; adding the trailer to the CRC would re-break v1.0
  bytes. Trailer integrity is protected by (a) its single-
  byte domain restricting values to {0,1,2} — any other
  value fails closed, (b) the BACE envelope's own CRC and
  MAC when `bace_envelope > 0`.
- Feature-flag reject: a v1 reader encountering
  `BACE_PAGE_ENVELOPE` set in `ALL_SUPPORTED_V1` already
  fails closed because `is_subset_supported` returns false
  for unknown bits.

### 3.2 Writer (`SegmentWriter::new_with_bace`)

```rust
pub struct SegmentWriter { ... }

impl SegmentWriter {
    pub fn new(file: File, options: SegmentOptions) -> BradianResult<Self>;
    pub fn new_with_bace(
        file: File,
        options: SegmentOptions,
        wiring: BaceWiring,
    ) -> BradianResult<Self>;
}
```

Per-page write path:

1. Encode via the inner `CodecId` codec, same as today.
2. If BACE wiring is attached AND the resolved outer codec is
   non-RAW:
   - Compress the inner codec output via the BACE codec's
     `encode()`.
   - Wrap the compressed bytes in a BACE v1 envelope with
     `codec_id` = BACE codec id (ZSTD=49 or RAW=48).
   - Write envelope bytes as the page's body.
   - Write validity bitmap verbatim after the body.
   - Append one trailer byte `0x01` (bace_envelope = 1).
   - Set `FeatureFlags::BACE_PAGE_ENVELOPE` in the segment
     header.
3. If BACE wiring is attached AND the resolved outer codec is
   RAW:
   - Encode the page without an outer BACE wrapper (the
     writer could emit a RAW BACE envelope, but that just
     adds 36 bytes of overhead for no compression benefit).
   - Append trailer byte `0x00` (legacy semantics).
4. If no wiring attached: identical to today's v1.0 writer.
   No trailer byte. Flag bit stays clear.

### 3.3 Reader (`SegmentReader::open_with_bace`)

```rust
impl SegmentReader {
    pub fn open(path: &Path) -> BradianResult<Self>;
    pub fn open_with_bace(path: &Path, wiring: BaceWiring) -> BradianResult<Self>;
}
```

Reader flow:

1. Parse magic + major + minor version. Reject major != 1.
2. Parse feature flags. If `BACE_PAGE_ENVELOPE` is set AND no
   wiring is attached, fail closed with a specific error
   analogous to ADR 0065's `BaceOpenWithoutBaceWiring`:
   `DerSegmentBaceRequired`.
3. Parse footer, iterate pages.
4. For each page with `bace_envelope = 1`, read the envelope
   under `read_envelope` with
   `max_decoded_length` = resolved policy cap,
   `max_encoded_length` = `payload_len`. Dispatch to the
   matching BACE codec's `decode()`.
5. For each page with `bace_envelope = 0`, use the legacy
   path unchanged.
6. For `bace_envelope >= 3`, fail closed (unknown envelope).
7. For `bace_envelope = 2`, fail closed with
   `DerSegmentBacaUnsupported` until a later ADR consumes
   BACA.

### 3.4 Cross-crate dependency

`bradian-storage/Cargo.toml` gains:

```toml
bradian-relational = { path = "../bradian-relational" }
```

Imports used:
- `bradian_relational::bace::codec::zstd::ZSTD_ID`
- `bradian_relational::bace::codec::raw::RAW_ID`
- `bradian_relational::bace::codec::{Codec, CodecId}`
- `bradian_relational::bace::envelope::{EnvelopeCaps, EnvelopeFlags, EnvelopeHeader, ENVELOPE_HEADER_SIZE, ENVELOPE_TRAILER_SIZE, read_envelope, write_envelope}`
- `bradian_relational::bace::policy::{PolicyKey, PolicyService, ScopeChain}`
- `bradian_relational::bace::registry::CodecRegistry`
- `bradian_relational::row_store::BaceWiring`

**This coupling is non-trivial.** It flips the historical
boundary that `bradian-relational` (the relational engine)
could depend on `bradian-storage` (the analytical engine) but not
the other way around. The coupling is actually *away* from
`bradian-relational` — `bradian-storage` depends on it. If this
is objectionable, the alternative is to move the BACE
subsystem to its own crate (`bradian-bace`) that both
engines consume. See §4 and §5 Q2.

### 3.5 Compliance obligations (implementation test checklist)

1. `SegmentWriter::new_with_bace` with ZSTD-Balanced wiring
   emits a v1.1 segment where `BACE_PAGE_ENVELOPE` flag is
   set and every page has a `bace_envelope=1` trailer byte.
2. `SegmentReader::open_with_bace` round-trips such a
   segment's rows.
3. Legacy `SegmentWriter::new` continues to produce v1.0
   segments (no flag, no trailer, unchanged bytes).
4. Legacy `SegmentReader::open` reads v1.0 segments
   unchanged.
5. Legacy `SegmentReader::open` on a v1.1 segment fails
   closed because `BACE_PAGE_ENVELOPE` is not in
   `ALL_SUPPORTED_V1`.
6. `SegmentReader::open_with_bace` on a legacy v1.0 segment
   succeeds (wiring is tolerated, pages have no envelope).
7. Tampered envelope bytes in a BACE-wrapped page fail with
   a BACE-layer error, not a silent incorrect read.
8. Policy cap `compression.max_decoded_page_bytes` below a
   page's declared `uncompressed_len` fails BEFORE
   allocation.
9. A v1.1 segment whose per-page trailer byte is `2` (BACA)
   fails `DerSegmentBacaUnsupported` cleanly.
10. A v1.1 segment whose trailer byte is `>= 3` fails
    `Corruption` cleanly.

### 3.6 Suggested implementation sub-slices

Given the cross-crate scope, if accepted, I recommend an
R.x-style implementation workflow analogous to ADR 0065:

- **R.1** — Cross-crate dep + the two new entry point
  signatures (`new_with_bace`, `open_with_bace`) returning
  the legacy behavior unchanged. No format change yet.
  Proves the dependency direction compiles.
- **R.2** — `.brcs` v1.1 format bits: `SEGMENT_VERSION_MINOR
  = 1`, `BACE_PAGE_ENVELOPE` flag, per-page trailer byte,
  `PageMeta::bace_envelope` field (if we revisit Option C
  vs A/B at implementation time). Reader tolerates v1.1 but
  no writer emits yet.
- **R.3** — Writer emits BACE envelopes under wiring.
- **R.4** — Reader decodes BACE envelopes; all §3.5 tests.

## 4. Alternatives considered

**A. Move BACE to its own crate (`bradian-bace`).** Cleaner
dependency direction — both `bradian-relational` and
`bradian-storage` depend on `bradian-bace` separately. Rejected
for v1 ADR scope: `bradian-relational::bace` is already
depended on by every consumer within that crate; splitting
it to a sibling now is a separate refactor. Future ADR can
split if coupling becomes a problem.

**B. Major version bump (`.brcs` v2) with no v1.0 backward
compat.** Simpler format. Rejected: operators would be
forced to compact every segment to upgrade, a risky mass-
rewrite.

**C. In-footer PageMeta field with `#[serde(default)]` via
a custom bincode deserializer.** Rejected: fragile bincode
semantics and likely to regress.

**D. Encode the `bace_envelope` tag inside the first byte of
the page body.** Rejected: conflates envelope framing with
BACE magic, and a v1.0 reader reading a v1.1 page would
mis-parse the first byte as valid payload.

**E. Skip `.brcs` BACE entirely; consume ZSTD at the
analytical layer directly.** Rejected: loses the uniform-
codec-governance story, the bounded-decode cap, and the
future BACA tamper-evidence path.

## 5. Open design questions (owner review)

1. **Per-page trailer byte vs bincode PageMeta field.**
   Draft: trailer byte (Option C) for v1.0 compat.
   Confirm.
2. **Cross-crate dep: `bradian-storage → bradian-relational`.**
   Draft: YES, direct `path = "../bradian-relational"`
   addition. Alternative: carve `bradian-bace` out as a
   sibling crate. Confirm the coupling direction.
3. **Minor version bump (1.1) vs major (2.0).** Draft: minor
   1.1 so v1.0 readers reject v1.1 via the feature-flag
   check and v1.1 readers accept both. Confirm.
4. **Policy scope.** Draft:
   `PolicyKey::CompressionMaxDecodedPageBytes` resolved at
   `ScopeChain::baseline_only()`, same as ADR 0065.
   Confirm.
5. **Writer default when wiring absent.** Draft: identical
   to today's v1.0 (no flag, no trailer). Confirm.
6. **BACA reservation in `bace_envelope`.** Draft: value
   `2` reserved; writer never emits; reader fails closed.
   Confirm.
7. **R.x sub-slice workflow.** Draft: R.1–R.4 as outlined
   in §3.6 if accepted. Confirm.

## 6. Impact on invariants

- **INV-001 commit correctness:** N/A — `.brcs` is a durable
  analytical format with no live transactions.
- **INV-002 one authoritative state:** no change. Legacy and
  BACE-wrapped segments coexist; neither hides the other.
- **INV-003 no authorization gap:** no change. BACE envelopes
  carry no authorization context; SQL authorization is
  consistent per CLAUDE.md §6 whether or not the segment is
  BACE-wrapped.
- **INV-004 no unsafe automatic availability:** N/A (local).
- **INV-005 recoverable lifecycle:** additive. No existing
  bytes are rewritten; v1.0 segments stay v1.0.

## 7. Dependencies this ADR would add

- `bradian-storage` → `bradian-relational` (new path dep). The
  biggest architectural item in this ADR.
- No new workspace crate pins.

## 8. What this ADR explicitly does NOT do

- Rename `.brcs` or any `dap-*` crate.
- Change the inner codec set.
- Add BACA / authenticated envelopes to `.brcs` production
  writes.
- Add encryption.
- Rewrite any existing v1.0 segment.
- Extend `bradian-admin` CLI for `.brcs` BACE.
- Touch the `.brcs` partition WAL, manifest, zone-map, or
  compaction code beyond the writer/reader entry points.
- Make a tamper-evidence security claim (that comes with
  the BACA carrier-integration ADR + CLAUDE.md §6 evidence).

## 9. Acceptance evidence required before marking Accepted

1. ADRs 0060–0075 accepted — **satisfied**.
2. Owner sign-off on §5 open design questions, especially
   Q1 (trailer byte vs serde field) and Q2 (cross-crate dep
   direction).
3. Implementation satisfying every item in §3.5, delivered
   as the R.1–R.4 sub-slices from §3.6 if accepted.
4. No regressions in `bradian-storage` test suite or any
   consumer tests (`bradian-planner`, `bradian-views`, etc.).

Only then can this ADR move from **PROPOSED** to **ACCEPTED**.
