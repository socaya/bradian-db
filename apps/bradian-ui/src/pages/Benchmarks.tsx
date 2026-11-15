// Benchmarks — historical comparisons and regressions.

import { EmptyState } from '@/components/States';
import { Pill } from '@/components/Pill';

interface BenchResult {
  commit: string;
  started: string;
  bench: string;
  p50_ms: number;
  p95_ms: number;
  p99_ms: number;
  deltaPct: number;
}

const RECENT: BenchResult[] = [
  { commit: '228c1ab', started: '2026-11-11 18:42', bench: 'scan_10k',               p50_ms: 3.4,  p95_ms: 4.0,  p99_ms: 4.3,  deltaPct: 0 },
  { commit: '228c1ab', started: '2026-11-11 18:42', bench: 'filter_aggregate/sum_by_ou_10k', p50_ms: 5.2,  p95_ms: 6.1,  p99_ms: 6.4,  deltaPct: 0 },
];

export function BenchmarksPage() {
  return (
    <div className="stack">
      <div>
        <h1>Benchmarks</h1>
        <p className="page-sub">
          Criterion runs captured via <code className="mono">scripts/bench.sh</code>.
          Each row carries the commit, dataset size and percentile latencies. Deltas
          are relative to the previous commit's run on the same hardware.
        </p>
      </div>

      {RECENT.length === 0 ? (
        <EmptyState
          title="No bench results recorded"
          description="Run `./scripts/bench.sh` to capture raw criterion output; the aggregator that surfaces them here is next."
        />
      ) : (
        <section className="panel">
          <table className="table">
            <thead>
              <tr>
                <th>Commit</th>
                <th>Bench</th>
                <th className="num">p50 ms</th>
                <th className="num">p95 ms</th>
                <th className="num">p99 ms</th>
                <th className="num">Δ vs prev</th>
                <th>Started</th>
              </tr>
            </thead>
            <tbody>
              {RECENT.map((r, i) => (
                <tr key={i}>
                  <td className="mono">{r.commit}</td>
                  <td>{r.bench}</td>
                  <td className="num mono">{r.p50_ms.toFixed(2)}</td>
                  <td className="num mono">{r.p95_ms.toFixed(2)}</td>
                  <td className="num mono">{r.p99_ms.toFixed(2)}</td>
                  <td className="num">
                    <Pill
                      tone={Math.abs(r.deltaPct) < 5 ? 'ok' : r.deltaPct > 0 ? 'warn' : 'info'}
                    >
                      {r.deltaPct > 0 ? '+' : ''}
                      {r.deltaPct.toFixed(1)}%
                    </Pill>
                  </td>
                  <td className="mono">{r.started}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}
    </div>
  );
}
