// Platform Overview — ingest lag, site freshness, query p95, CPU/memory/disk, alerts
//.

import { useQuery } from '@tanstack/react-query';
import { api } from '@/api/client';
import { Pill } from '@/components/Pill';
import { FailedState, LoadingLine } from '@/components/States';

export function OverviewPage() {
  const health = useQuery({ queryKey: ['health'], queryFn: api.health });
  const jobs = useQuery({ queryKey: ['jobs'], queryFn: api.listJobs, refetchInterval: 10_000 });
  const products = useQuery({ queryKey: ['data-products'], queryFn: api.listDataProducts });

  if (health.isLoading) return <LoadingLine label="Reading engine health…" />;
  if (health.error) {
    return (
      <FailedState
        message={(health.error as Error).message}
        retry={() => health.refetch()}
      />
    );
  }

  const runningJobs = (jobs.data ?? []).filter((j) => j.status === 'RUNNING').length;
  const failedJobs = (jobs.data ?? []).filter((j) => j.status === 'FAILED').length;

  return (
    <div className="stack">
      <div>
        <h1>Platform Overview</h1>
        <p className="page-sub">
          Live engine health, ingest watermarks, query throughput and background work.
          Every value states its source — see provenance on individual screens for detail.
        </p>
      </div>

      <section className="tile-grid">
        <Tile label="Engine" value={<Pill tone="ok">{health.data?.status}</Pill>} sub="synchronous probe" />
        <Tile
          label="Partitions open"
          value={health.data?.partitions_open ?? 0}
          sub="loaded in process"
        />
        <Tile
          label="Queries served"
          value={health.data?.query_history_size ?? 0}
          sub="since process start"
        />
        <Tile
          label="Running jobs"
          value={runningJobs}
          sub={`${failedJobs} failed since start`}
        />
        <Tile
          label="Data products"
          value={products.data?.length ?? 0}
          sub="registered in catalogue"
        />
        <Tile
          label="Catalog revision"
          value={
            <span className="mono">
              {health.data?.catalog_revision.slice(0, 12) ?? '—'}
            </span>
          }
          sub="versioned metadata"
        />
      </section>

      <section className="panel">
        <div className="panel-head">
          <h2>Recent jobs</h2>
          <a href="/history">Query History →</a>
        </div>
        <div className="panel-body" style={{ padding: 0 }}>
          <table className="table">
            <thead>
              <tr>
                <th>ID</th>
                <th>Kind</th>
                <th>Status</th>
                <th>Duration</th>
                <th>Submitted</th>
              </tr>
            </thead>
            <tbody>
              {(jobs.data ?? []).slice(-10).reverse().map((j) => (
                <tr key={j.id}>
                  <td className="mono">{j.id}</td>
                  <td>{j.kind}</td>
                  <td>
                    <Pill tone={jobToneFor(j.status)}>{j.status.toLowerCase()}</Pill>
                  </td>
                  <td className="mono">
                    {j.finished_at_ms && j.started_at_ms
                      ? `${j.finished_at_ms - j.started_at_ms} ms`
                      : '—'}
                  </td>
                  <td className="mono">
                    {new Date(j.submitted_at_ms).toLocaleTimeString()}
                  </td>
                </tr>
              ))}
              {(jobs.data ?? []).length === 0 && (
                <tr className="row-empty">
                  <td colSpan={5}>No jobs submitted yet.</td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </section>
    </div>
  );
}

function Tile({
  label,
  value,
  sub,
}: {
  label: string;
  value: React.ReactNode;
  sub?: string;
}) {
  return (
    <div className="tile">
      <div className="label">{label}</div>
      <div className="value">{value}</div>
      {sub && <div className="sub">{sub}</div>}
    </div>
  );
}

export function jobToneFor(status: string): 'ok' | 'warn' | 'err' | 'info' | 'neutral' {
  switch (status) {
    case 'SUCCEEDED':
      return 'ok';
    case 'RUNNING':
      return 'info';
    case 'FAILED':
      return 'err';
    case 'CANCELLED':
      return 'warn';
    default:
      return 'neutral';
  }
}
