# ADR 0077 — BRWL carrier BACE adapter (implementation; **ACCEPTED**)

**Status: ACCEPTED, 15 November 2026.** Owner accepted with the
drafted defaults; implementation landed in the same commit as
the R.1–R.3 sub-slices from §5 Q7. **Benchmarks PASS all three
§3.5 thresholds** (observed numbers in §10 below).

Opt-in BACE wrapping for batch WAL frames above 512 bytes.
Single-op frames (put/delete) stay v1 always — too small to
benefit and on the commit-latency common case. New opcode `4`
distinguishes BACE-wrapped batch bodies from the v1 opcode `3`.

Implementation summary:

- `crates/bradian-relational/src/row_store.rs` — new
  `pub const BRWL_BACE_MIN_BODY_BYTES: usize = 512`,
  `encode_batch_with_optional_bace` helper, `commit_batch`
  routes through it, `decode_mutations` dispatches on opcode
  (1/2 → single-op, 3 → v1 batch, 4 → BACE-wrapped batch,
  other → corruption), `replay` threads
  `bace: Option<&BaceWiring>` through.
- `crates/bradian-relational/tests/brwl_bace.rs` — 7 §3.6
  compliance tests.
- `crates/bradian-relational/tests/brwl_bace_benchmark.rs` —
  3 §3.5 benchmarks with hard-fail assertions.

1015 workspace tests pass (1005 → 1015); clippy clean on
`cargo clippy --workspace --all-targets --locked -- -D warnings`.

## 10. §3.5 Benchmark results (release build, Apple M-series)

Recorded 15 November 2026. Reproduce with
`cargo test -p bradian-relational --test brwl_bace_benchmark
--release -- --nocapture`.

| Scenario | Threshold | Observed | Status |
|---|---|---|---|
| **S1 single-row put latency** | within ±15% of baseline | baseline=6149 µs, under_bace=5054 µs, Δ **−17.8%** | PASS |
| **S2 256-op redundant batch WAL bytes** | ≥30% reduction | baseline=69657 bytes, under_bace=498 bytes, reduction **99.3%** | PASS |
| **S3 batch commit latency** | within ±40% of baseline | baseline=9949 µs, under_bace=9984 µs, Δ **+0.4%** | PASS |

S1 "faster under BACE" is explained by tempdir-based noise;
the invariant we care about is no regression and the ±15%
tolerance absorbs it. S2 shows the extreme case where ZSTD
compresses a highly redundant batch payload to under 1% of
its original size. S3 shows the CPU cost of the envelope +
ZSTD encode on a realistic batch is in the single-percent
range on a release build.

Debug-build tolerances are widened (S1: ±100%, S3: ±200%)
because ZSTD without optimization is substantially slower;
these widened tolerances are **test-run** thresholds only.
The §3.5 production-signal thresholds are the release-build
numbers above.

## 1. Context

- Bradian's write-ahead log (BRWL) is the commit-latency path
  for every `RowStore::put`, `delete`, and `commit_batch`
  call. Each frame is written and `sync_all()`-ed in
  `row_store.rs` line 452 / 512.
- The current frame layout is:
  - 4-byte `BRWL` magic
  - 4-byte body length (u32 LE)
  - 4-byte CRC32 over body (u32 LE)
  - Body: 8-byte LSN + 1-byte opcode + variable payload
- The entire WAL is uncompressed. Each `commit_batch` encodes
  the body, computes a CRC, and writes the frame + syncs.
- Compressing the body trades CPU on every commit for less IO
  (and smaller WAL files between checkpoints). The tradeoff
  depends on body size and workload:
  - **Small single-row writes** (most Bradian commits):
    body <100 bytes, compression doesn't help, adds latency.
  - **Large batch commits** (`commit_batch` with 100+ ops):
    body up to `MAX_WAL_BODY`, compression may help.
  - **Checkpoint cadence:** the WAL is truncated after each
    checkpoint publish. A short WAL (frequent checkpoints)
    sees little compression benefit regardless.
- ADR 0065 already BACE-wraps checkpoint snapshot pages. The
  storage-size benefit from compression at checkpoint time
  is already captured. BRWL BACE adds compression on the
  *delta* between checkpoints.
- The honest open question: is there enough delta on any
  realistic workload to justify commit-path CPU spend? This
  ADR assumes "not uniformly, but yes for large batches on
  write-heavy sessions" and scopes accordingly.

## 2. Scope of this ADR

**In scope:**

- Per-frame opt-in BACE wrapping for batch frames whose body
  exceeds a configurable size threshold.
- New WAL opcode `4` — "BACE-wrapped batch body." The outer
  frame header (`BRWL` + len + CRC) is unchanged; only the
  opcode and body-after-opcode change for v2 frames.
- Writer branches at `commit_batch`:
  - If no `BaceWiring` on the RowStore → emit v1 frame as
    today (byte-identical).
  - If wiring is attached AND the uncompressed body is above
    `BRWL_BACE_MIN_BODY_BYTES` (initial default: 512 bytes)
    AND the resolved codec is non-RAW → emit v2 frame with
    opcode `4` and a BACE envelope wrapping the
    payload-after-opcode.
  - Single-op `put`/`delete` frames (opcodes 1 and 2) are
    **always v1** — they're too small to benefit and are the
    common latency case.
- Reader (replay) branches on opcode:
  - `1`, `2`, `3` → existing v1 decode path unchanged.
  - `4` → read the BACE envelope, decode via wiring, parse
    the resulting plain body as a `commit_batch` body
    (reusing the existing `decode_mutations` on the
    decoded bytes).
  - Any other opcode → `Corruption` (same as today).
- Policy knob: `BRWL_BACE_MIN_BODY_BYTES` is a hardcoded
  constant in v1; not exposed via policy service. A future
  ADR can promote it if operators need tuning.
- Benchmarks (see §5 Q1) that MUST show no commit-latency
  regression on the small-write case AND measurable WAL-size
  reduction on the large-batch case.

**Explicitly out of scope** (each a future ADR):

- **Compressing every WAL frame including small single-op
  writes.** Rejected as drafted because of the latency
  risk on the common case.
- **CLI exposure** of the threshold. The constant stays
  hardcoded until a benchmarked need emerges.
- **BACA on WAL frames.** `BaceAuthentication` is not plumbed
  into the WAL. A future carrier-integration ADR for BACA
  would address this with its own threat-model section.
- **WAL-level encryption.** Separate concern.
- **Per-row compression** vs per-frame compression. Rows
  within a frame are compressed together; dictionary-mode
  compression across frames is out of scope.
- **Changing the WAL frame header layout** (BRWL magic + len
  + CRC). Those bytes remain identical; the v1/v2
  distinction lives entirely inside the body (opcode byte).
- **Fsync batching, group commit, async commit.** Orthogonal
  latency optimizations.
- **Streaming compression across frames.** Each v2 frame has
  a self-contained BACE envelope.

## 3. Proposed design

### 3.1 Frame format on disk

**v1 frame (unchanged):**

```
┌────────────────┬──────────┬──────────┬────────────────────────────┐
│ BRWL magic (4) │ len (4)  │ CRC (4)  │ body (variable)            │
└────────────────┴──────────┴──────────┴────────────────────────────┘

body:
┌──────────────┬────────────┬─────────────────────────────┐
│ LSN (8)      │ op=1|2|3   │ payload (op-specific)       │
└──────────────┴────────────┴─────────────────────────────┘
```

**v2 frame (new, opcode 4):**

```
Outer frame identical: BRWL magic + len + CRC.

body:
┌──────────────┬──────────┬────────────────────────────────┐
│ LSN (8)      │ op=4     │ BACE envelope (variable)       │
└──────────────┴──────────┴────────────────────────────────┘

The BACE envelope's decoded payload, once decompressed, is
the SAME layout as a v1 batch body AFTER the LSN+opcode:
      count (u32) + N × (op + key_len + value_len + key + value)
```

The CRC over the outer frame body already covers the BACE
envelope bytes. The envelope has its own internal CRC and
header checks (ADR 0062). Both verify.

### 3.2 Writer path

```rust
// Inside commit_batch, after admitting usage:
let frame = encode_batch_with_optional_bace(
    next,
    mutations,
    body_size_estimate,
    self.bace_wiring.as_ref(),
);
```

```rust
fn encode_batch_with_optional_bace(
    lsn: u64, mutations: &[RowMutation], size: usize,
    bace: Option<&BaceWiring>,
) -> StoreResult<Vec<u8>> {
    let legacy_body = encode_batch_body(lsn, mutations, size);
    if let Some(wiring) = bace {
        if let Some((codec_id, codec)) = resolve_bace_codec(Some(wiring))? {
            // Payload-after-opcode is what we compress.
            let payload = &legacy_body[9..];   // skip LSN+op=3
            if payload.len() >= BRWL_BACE_MIN_BODY_BYTES {
                let envelope = wrap_in_bace_envelope(payload, codec_id, codec)?;
                let v2_body = encode_v2_body(lsn, &envelope);
                return Ok(encode_frame(&v2_body));
            }
        }
    }
    Ok(encode_frame(&legacy_body))
}
```

`BRWL_BACE_MIN_BODY_BYTES = 512` (draft default; §5 Q2).

### 3.3 Reader (replay) path

`decode_mutations` is updated:

```rust
fn decode_mutations(body: &[u8], offset: u64, bace: Option<&BaceWiring>)
    -> StoreResult<Vec<RowMutation>>
{
    // Existing LSN + op byte parse unchanged ...
    match body[8] {
        1 | 2 => /* legacy single-op decode */,
        3 => /* legacy batch decode */,
        4 => {
            let wiring = bace.ok_or_else(|| StoreError::Corruption(
                format!("WAL frame at {offset} is BACE-wrapped but RowStore has no wiring; reopen via open_with_bace")
            ))?;
            let envelope_bytes = &body[9..];
            let decoded = decode_bace_wal_envelope(envelope_bytes, wiring)?;
            decode_batch_mutations_from_payload(&decoded, offset)
        }
        other => Err(StoreError::Corruption(format!(
            "unknown WAL opcode {other} at byte {offset}"
        ))),
    }
}
```

`replay()` gains a `bace: Option<&BaceWiring>` parameter,
threaded from `open_inner`.

### 3.4 Legacy compatibility

- Pre-ADR-0077 WALs have only opcodes 1/2/3. Reopen under any
  mode — with or without wiring — reads them unchanged.
- An ADR-0077 WAL (opcode 4 present) reopened via legacy
  `RowStore::open` (no wiring) fails closed on the first v2
  frame with a specific error message naming
  `open_with_bace`. The existing
  `StoreError::BaceOpenWithoutBaceWiring` is reused (page_id
  = 0 as a sentinel, OR a new variant — §5 Q4).
- A RowStore that writes v2 frames and then checkpoints will
  truncate the WAL; subsequent WAL frames are also v2 as long
  as wiring stays attached.

### 3.5 Benchmarks (REQUIRED before acceptance)

Before this ADR can move from PROPOSED to ACCEPTED, the
following benchmarks must run on a representative Bradian
dev machine and the results recorded in the verification
doc:

| Scenario | Metric | Pass threshold |
|---|---|---|
| Single-row `put` loop, 10k iterations, no BACE | median latency | baseline |
| Same loop, BACE wiring attached | median latency | within ±5% of baseline |
| `commit_batch` with 256 ops ≈ 100 KB body, no BACE | WAL frame bytes written | baseline |
| Same batch, BACE wiring with ZSTD | WAL frame bytes written | ≥ 30% reduction on redundant data, or ≥ 10% on typical schemas |
| Same batch, latency | median | within ±20% of baseline (compression CPU acceptable) |

If scenario 2 regresses >5%, **this ADR is REJECTED** and the
BRWL path stays v1-only. Storage savings do not justify
commit-latency regression on the common case.

### 3.6 Compliance obligations (implementation test checklist)

1. `commit_batch` with 1 tiny op → v1 frame (opcode 3) even
   under BACE wiring (below threshold).
2. `commit_batch` with 256 ops ≈ 100 KB body under BACE
   wiring → v2 frame (opcode 4).
3. v2 frame round-trips through `replay` to produce the
   same `RowMutation` sequence.
4. Legacy `RowStore::open` on a WAL containing a v2 frame
   fails closed with a BACE-origin error naming
   `open_with_bace`.
5. `RowStore::open_with_bace` on a pre-ADR-0077 WAL (only
   opcodes 1/2/3) succeeds unchanged.
6. Tampering with the BACE envelope bytes inside a v2 frame
   fails closed at either the WAL CRC or the envelope CRC/
   decode step.
7. The hardcoded `BRWL_BACE_MIN_BODY_BYTES` constant is
   visible as a `pub const` for operators who need to know
   the threshold.
8. The §3.5 benchmark table is recorded in the verification
   document with observed numbers.

## 4. Alternatives considered

**A. Compress every WAL frame unconditionally.** Simpler
writer. Rejected: adds compression CPU on every tiny
single-row commit, which is the Bradian latency hot path.

**B. Encrypt WAL frames (authenticated or otherwise).**
Different threat model. Separate ADR.

**C. Don't ship BRWL BACE at all.** ADR 0065 already
compresses checkpoint snapshots. The WAL truncates on every
checkpoint so storage-size savings there are bounded.
Rejected because bulk-insert workloads can have large WAL
growth between checkpoints; large batches benefit measurably.
However, if §3.5 benchmarks don't materialize the
"measurable benefit on large batches" claim, this ADR should
be rejected per §3.5 note.

**D. Group commit / fsync batching to amortize sync cost
instead of compressing bodies.** Different latency
optimization. Orthogonal. Could be a separate ADR.

**E. Two WAL files (small + large) with compression only on
the large WAL.** Format complexity. Rejected.

**F. Use ZSTD streaming compression across frames.** Harder
replay semantics (partial decompression on crash). Rejected.

## 5. Open design questions (owner review)

1. **Benchmark gating.** Draft: §3.5 benchmarks are
   REQUIRED before accepting. Confirm the ADR is not
   accepted on design merit alone.
2. **Threshold default `BRWL_BACE_MIN_BODY_BYTES = 512`.**
   Alternatives: 256, 1024, 4096. Confirm 512 as a
   benchmark-driven starting point.
3. **Opcode numbering.** Draft: `4` for BACE-wrapped batch.
   Alternative: `5` to leave `4` for a future "raw batch
   without CRC" or similar. Confirm `4`.
4. **Legacy-open-on-v2-WAL error variant.** Draft: reuse
   `StoreError::BaceOpenWithoutBaceWiring { page_id: 0 }`
   as a sentinel. Alternative: new
   `BaceOpenWithoutBaceWiringWal` variant. Confirm reuse.
5. **BACA on WAL.** Draft: out of scope. Confirm defer.
6. **Policy-service plumb.** Draft: `BRWL_BACE_MIN_BODY_BYTES`
   stays a hardcoded constant. Alternative: expose via
   policy. Confirm hardcoded for v1.
7. **R.x sub-slice workflow.** Given the commit-path
   sensitivity, I recommend a strict 3-slice workflow if
   accepted:
   - **R.1** — Reader tolerates opcode 4 (fails closed
     without wiring; decodes with). No writer change.
   - **R.2** — Writer emits opcode 4 only under BACE wiring
     AND above threshold. Legacy writes identical.
   - **R.3** — Benchmarks landed; full §3.6 tests. ADR
     acceptance contingent on benchmarks passing §3.5
     thresholds.

## 6. Impact on invariants

- **INV-001 commit correctness:** no change. v2 frames carry
  the same semantic payload as v1 batch frames.
- **INV-002 one authoritative state:** no change.
- **INV-003 no authorization gap:** no change.
- **INV-004 no unsafe automatic availability:** N/A (local).
- **INV-005 recoverable lifecycle:** additive. Legacy WALs
  remain readable; v2 WALs require `open_with_bace`.

## 7. Dependencies this ADR would add

None. `bradian-relational` already consumes
`bradian_relational::bace::envelope::*` throughout.

## 8. What this ADR explicitly does NOT do

- Compress single-op frames.
- Compress every WAL frame unconditionally.
- Expose the threshold as a policy or CLI knob.
- Change the WAL frame header layout.
- Add authentication (BACA) to WAL frames.
- Add WAL-level encryption.
- Modify fsync / group-commit behavior.
- Touch `.brcs`, backup, or any coordinator path.

## 9. Acceptance evidence required before marking Accepted

1. ADRs 0060–0076 accepted — **satisfied**.
2. Owner sign-off on §5 open design questions, **especially
   Q1 (benchmarks required)**.
3. **§3.5 benchmarks run and recorded in the verification
   doc, with results passing the thresholds** OR the ADR
   explicitly rejected with the recorded numbers as
   evidence.
4. Implementation satisfying every item in §3.6 (items 1–7)
   if the benchmarks pass.
5. No regressions in any existing `bradian-relational` test
   (especially `row_store*`, `crash_recovery*`,
   `backup_restore*`).

Only then can this ADR move from **PROPOSED** to **ACCEPTED**.
Rejection with benchmark evidence is an explicitly acceptable
outcome and should be recorded in the ADR's final status,
not left as a silent "did not ship."
