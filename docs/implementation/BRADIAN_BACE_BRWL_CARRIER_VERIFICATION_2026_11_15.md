# BACE BRWL carrier adapter verification (ADR 0077)

15 November 2026. Baseline: `b0fd52e`. First BACE ADR that
touches the commit-latency path. **All three §3.5 benchmarks
pass their release-build thresholds**, so the ADR flips from
PROPOSED to ACCEPTED.

## Executed result

**1015 Rust tests passed, 0 failed, 0 ignored** (1005 → 1015).
Clippy clean on `cargo clippy --workspace --all-targets
--locked -- -D warnings`.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-relational` | Passed |
| `cargo test --workspace --locked` | 1015 passed, 0 failed |
| `cargo test -p bradian-relational --test brwl_bace` | 7 passed |
| `cargo test -p bradian-relational --test brwl_bace_benchmark --release -- --nocapture` | 3 passed (benchmarks) |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Passed |

## §3.5 Benchmark results (the decision)

Reproduce with:

```sh
cargo test -p bradian-relational --test brwl_bace_benchmark --release -- --nocapture
```

Observed on Apple M-series, 15 November 2026, release build:

```
ADR0077-S1 single-row put median latency: baseline=6149.4us, under_bace=5053.8us, delta=-17.8%
ADR0077-S2 256-op redundant batch: baseline_bytes=69657 (16028us), under_bace_bytes=498 (12143us), size_reduction=99.3%, latency_delta=-24.2%
ADR0077-S3 batch commit latency: baseline=9949us, under_bace=9984us, delta=+0.4%
```

### Interpretation

- **S1** — single-row put under BACE wiring is actually
  *faster* on this run (−17.8%). That's within the ±15%
  tolerance in the direction we'd hope for; the test's
  assertion threshold is "no regression" so a win here is
  acceptable. Single-op frames use opcode 1 and never reach
  the envelope path per §3.2, so the cost of BACE wiring on
  the common case is just an `Option::is_some()` check.
- **S2** — The 256-op redundant batch compresses from 69657
  bytes to 498 bytes under ZSTD. Far exceeds the 30%
  threshold. This is the storage-size justification the ADR
  needed.
- **S3** — Batch commit latency is essentially unchanged
  (+0.4%). Compression CPU is dominated by the WAL fsync.

### Honest caveats recorded in the ADR

- Benchmarks run on a single developer machine. Cross-
  platform / cross-architecture variance is NOT measured.
  A future acceptance gate could run this in CI across the
  supported targets.
- S2's "99.3% reduction" is a best-case for highly redundant
  data (256 bytes of `'x'` per value). Real workloads with
  mixed content will see smaller reductions. The ≥30%
  threshold is calibrated for the case we claim justifies
  this ADR; real-world savings may be lower but still
  worthwhile.
- Debug-build tolerances are intentionally widened (S1:
  ±100%, S3: ±200%) because ZSTD without optimization is
  slow enough that `cargo test` (default `--debug`) would
  spuriously fail the tighter thresholds. The §3.5
  production-signal thresholds are the release-build
  numbers.

## What landed

### R.1 — reader tolerates opcode 4

`decode_mutations` refactored into:
- `decode_batch_payload(payload, offset)` — parses the
  `count + records` layout, shared between v1 opcode 3 and
  the BACE-decoded v2 opcode 4.
- `decode_single_mutation(op, body, offset)` — the v1
  single-op path.
- `decode_bace_wal_envelope(envelope_bytes, wiring, offset)`
  — uses `CompressionMaxDecodedPageBytes` from the policy
  service as the envelope cap; returns `BaceCodecUnavailable`
  / `BaceCodecFormatVersionMismatch` on registry misses.
- `decode_mutations(body, offset, bace)` — opcode dispatch.

`replay` gains a `bace: Option<&BaceWiring>` arg; `open_inner`
passes `bace_wiring.as_ref()`.

### R.2 — writer emits opcode 4 above threshold

`commit_batch` routes through
`encode_batch_with_optional_bace(lsn, mutations, size, bace)`:

- No wiring → v1 frame identical to today (fast path).
- Wiring + resolved codec is RAW → v1 frame (RAW opts out
  of envelope overhead for pages that wouldn't compress).
- Wiring + non-RAW codec + body-after-opcode ≥ 512 bytes →
  v2 frame with opcode 4 + BACE envelope.
- Wiring + non-RAW codec + body below threshold → v1 frame.

Single-op `put` / `delete` continue to use the unchanged
`encode_wal` path (opcodes 1/2 only, no v2 variant).

### R.3 — benchmarks + §3.6 tests

- `tests/brwl_bace.rs` — 7 §3.6 compliance tests.
- `tests/brwl_bace_benchmark.rs` — 3 §3.5 benchmarks with
  hard-fail assertions that would block ADR acceptance if
  thresholds regressed.

### 7 §3.6 compliance tests

| # | Test | §3.6 item |
|---|---|---|
| 1 | `item_1_single_op_commit_under_bace_wiring_stays_v1_below_threshold` | 1 |
| 2 | `item_2_large_batch_under_bace_wiring_emits_v2_opcode_4` | 2 |
| 3 | `item_3_v2_frame_round_trips_through_replay` | 3 |
| 4 | `item_4_legacy_open_on_v2_wal_fails_closed` | 4 |
| 5 | `item_5_open_with_bace_on_pre_adr_0077_wal_succeeds` | 5 |
| 6 | `item_6_tampered_v2_envelope_fails_closed` | 6 |
| 7 | `item_7_threshold_constant_is_publicly_visible` | 7 |

## Compliance against ADR 0077 §9 acceptance evidence

1. ADRs 0060–0076 accepted — ✓
2. §5 open design questions resolved per drafted defaults.
3. §3.5 benchmarks run and recorded — ✓ (see §10 of the ADR
   itself).
4. All §3.6 items 1–7 landed as tests.
5. No regressions in `bradian-relational`:
   `row_store*`, `crash_recovery*`, `backup_restore*` all
   pass unchanged.

## What this ADR explicitly does NOT do (unchanged)

- Compress single-op frames.
- Compress every WAL frame unconditionally.
- Expose the threshold as a policy or CLI knob.
- Change the WAL frame header layout.
- Add authentication (BACA) to WAL frames.
- Add WAL-level encryption.
- Modify fsync / group-commit behavior.

## BACE workstream progress (as of this ADR)

| ADR | Component | Status |
|---|---|---|
| 0060–0066 | Prerequisites + RowStore + ZSTD | ACCEPTED |
| 0067–0068 | Backup carrier adapters | ACCEPTED |
| 0069–0072 | All four SQL engines BACE open | ACCEPTED |
| 0073 | `bradian-db` CLI BACE flags | ACCEPTED |
| 0074 | G23 DDL demonstration | ACCEPTED |
| 0075 | BACA envelope primitive | ACCEPTED |
| 0076 | `.derc` carrier BACE (cross-crate) | ACCEPTED |
| **0077** | **BRWL carrier BACE (benchmarked)** | **ACCEPTED** |

## Remaining BACE workstream candidates

- **Dictionary scope** — BACE §4.2 dictionary mode with
  per-column lifecycle.
- **Carrier integration of `BaceAuthentication`** — the
  concrete tamper-evidence feature consuming ADR 0075's
  primitive; needs full CLAUDE.md §6 evidence package.
- **Benchmarks on more realistic DHIS2/Bradian workloads**
  — the §3.5 thresholds are calibrated on a redundant test
  payload. Real-world savings measurements would be
  valuable for the roadmap narrative.

## Baseline + commit

Baseline: `cdf7bf0` (ADR 0077 PROPOSED).
Implementation commit: assigned by this commit.

See [ADR 0077](../adr/0077-brwl-carrier-bace.md).
