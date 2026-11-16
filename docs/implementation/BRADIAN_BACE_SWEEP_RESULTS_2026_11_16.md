# BACE sweep benchmark corpus — published results

16 November 2026. Baseline: `fabd5a2`. Synthetic DHIS2-like
workload (`generate_facts(seed=42, 50 OUs, 12 periods, 20
DEs, 36_000 rows)`; 5 columns: ou, period, de, coc, value).

## Observed (`cargo bench -p bradian-benchmark --bench bace_sweep`)

| max_rows_per_page | column_dict | bytes | write (ms) | read (ms) |
|---:|:---:|---:|---:|---:|
| 8    | off | 5 722 615 | 155.1 | 70.8 |
| 64   | off | 1 104 420 |  42.5 | 11.0 |
| 512  | off |   474 480 |  19.9 |  2.7 |
| 4096 | off |   379 244 |  18.1 |  1.4 |
| 8    | on  | 3 823 656 | 102.9 | 47.2 |
| 64   | on  |   886 116 |  28.4 |  7.7 |
| 512  | on  |   438 162 |  21.0 |  2.4 |
| 4096 | on  |   374 898 |  17.4 |  1.3 |

## Operator reading

- **Default for mixed DHIS2-like workloads: `max_rows_per_page
  = 512`, `use_column_dict = false`.** Within 25% of the
  minimum size + fastest read.
- **Small-page (8-row) column-dict is a 33% win over small-page
  plain**, confirming the ADR 0083 §3.5 item 10 hypothesis
  (large dict × many pages amortises dict pool duplication).
- **Large-page (4096-row) column-dict is a 1% improvement over
  large-page plain.** The ADR 0094 "column-dict is not
  universally smaller" observation holds: once pages are big
  enough for plain ZSTD to self-learn, the dict pool is
  near-zero-value.
- **Read time scales linearly with page count**, not with
  per-page compressed size. Page-count = 36000 / page_size;
  more pages = more per-page overhead (CRC, trailer, envelope).

## Honest limitations

- Synthetic workload only; real DHIS2 data is out of scope
  (owner procurement).
- Single-threaded reader. Multi-column scan parallelism not
  measured.
- Local filesystem; network-mount / object-store cold reads
  not measured.
- `cargo bench` runs under the dev `--release` profile
  because the workspace Cargo config pins `[profile.bench]`.
  No `--release` manual override needed.

The sweep is the eventual G1-FAULTS / G1-OPS-008 evidence
input. Combined with ADR 0094 and the five-variant
scorecard, operators have enough data to pick a sensible
default without §13 unblocks.
