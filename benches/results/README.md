# Benchmark results

Run `./scripts/bench.sh` from the workspace root. The script captures:

- git commit SHA
- rustc version
- host info (uname -snmr)
- core count
- raw criterion output

in `benches/results/<timestamp>_<commit>/`. Per spec §12, no latency number should be
published without the matching `context.json`.

## Baseline (DAP v0.1.0 bootstrap)

Captured on Apple Silicon (m-series) during the first development cycle.
Numbers are intended as a regression-detection baseline, not as a public benchmark.

| Workload | Rows | p50 | throughput |
|---|---|---|---|
| scan_10k | 10 000 | ~3.4 ms | ~2.9 M rows/s |
| filter_aggregate/sum_by_ou_10k | 10 000 | ~5.2 ms | ~1.9 M rows/s |

The v1 scan path materialises each column via `ColumnBuffer::value_at` row-by-row. The
spec target (interactive central aggregation p95 under 2 s on a 100M fact fixture) will
require the typed vectorized kernels tracked under OPT-002 / OPT-003 and multi-page
column iterators tracked under STO-004.
