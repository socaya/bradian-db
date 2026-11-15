// Query History — fingerprint, duration, scan bytes, spill, user scope.

import { useQuery } from '@tanstack/react-query';
import { api } from '@/api/client';
import { Pill } from '@/components/Pill';
import { LoadingLine } from '@/components/States';
import { jobToneFor } from './Overview';

export function QueryHistoryPage() {
  const jobs = useQuery({
    queryKey: ['jobs'],
    queryFn: api.listJobs,
    refetchInterval: 5_000,
  });

  if (jobs.isLoading) return <LoadingLine label="Reading history…" />;
  const sorted = [...(jobs.data ?? [])].sort((a, b) => b.submitted_at_ms - a.submitted_at_ms);

  return (
    <div className="stack">
      <div>
        <h1>Query History</h1>
        <p className="page-sub">
          Every submitted analytical job with status, timing and the principal that
          submitted it. Cancel pending runs or inspect failed ones.
        </p>
      </div>

      <section className="panel">
        <table className="table">
          <thead>
            <tr>
              <th>ID</th>
              <th>Kind</th>
              <th>Status</th>
              <th>Submitted by</th>
              <th>Submitted</th>
              <th>Duration</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {sorted.map((j) => (
              <tr key={j.id}>
                <td className="mono">{j.id}</td>
                <td>{j.kind}</td>
                <td>
                  <Pill tone={jobToneFor(j.status)}>{j.status.toLowerCase()}</Pill>
                </td>
                <td>{j.submitted_by ?? 'anonymous'}</td>
                <td className="mono">{new Date(j.submitted_at_ms).toLocaleString()}</td>
                <td className="mono">
                  {j.finished_at_ms && j.started_at_ms
                    ? `${j.finished_at_ms - j.started_at_ms} ms`
                    : '—'}
                </td>
                <td>
                  {j.status === 'RUNNING' && (
                    <button
                      className="btn sm"
                      onClick={() => api.cancelJob(j.id).then(() => jobs.refetch())}
                    >
                      Cancel
                    </button>
                  )}
                </td>
              </tr>
            ))}
            {sorted.length === 0 && (
              <tr className="row-empty">
                <td colSpan={7}>No queries recorded.</td>
              </tr>
            )}
          </tbody>
        </table>
      </section>
    </div>
  );
}
