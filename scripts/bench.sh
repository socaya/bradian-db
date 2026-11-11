#!/usr/bin/env bash
# Reproducible benchmark runner. Captures git revision, rustc version, host info and
# criterion output under `benches/results/<timestamp>/`.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
cd "$here"

commit="$(git rev-parse --short HEAD 2>/dev/null || echo "unknown")"
timestamp="$(date -u +"%Y%m%dT%H%M%SZ")"
out="benches/results/${timestamp}_${commit}"
mkdir -p "$out"

rustc_version="$(rustc --version)"
host="$(uname -snmr)"
cores="$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 'unknown')"

cat > "$out/context.json" <<EOF
{
  "commit": "$commit",
  "rustc_version": "$rustc_version",
  "host": "$host",
  "cores": "$cores",
  "started_at": "$timestamp",
  "notes": "${BENCH_NOTES:-}"
}
EOF

cargo bench -p dap-benchmark --bench scan_filter_aggregate -- \
  --warm-up-time 1 \
  --measurement-time 3 \
  --sample-size 20 \
  | tee "$out/raw.txt"

echo "Bench artifacts: $out"
