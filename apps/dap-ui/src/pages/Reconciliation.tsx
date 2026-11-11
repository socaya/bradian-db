// Reconciliation — site journal watermarks, gaps, conflicts and central acknowledgment
//.

import { EmptyState } from '@/components/States';
import { Pill } from '@/components/Pill';

interface SiteCheckpoint {
  siteId: string;
  ackedLsn: number;
  localLsn: number;
  lastAckMs: number;
}

const SITES: SiteCheckpoint[] = [];

export function ReconciliationPage() {
  return (
    <div className="stack">
      <div>
        <h1>Reconciliation</h1>
        <p className="page-sub">
          Edge sites with pending deltas, acknowledgement LSNs and reconciliation outcomes.
          A site is <em>healthy</em> when lag is small and no conflicts remain unresolved.
        </p>
      </div>

      {SITES.length === 0 ? (
        <EmptyState
          title="No sites have reported yet"
          description="Reconciliation activates when edge workers call the server's /sync endpoint. The in-process ReconciliationLog is live; the networked transport is the final DST-002 piece."
        />
      ) : (
        <section className="panel">
          <table className="table">
            <thead>
              <tr>
                <th>Site</th>
                <th className="num">Acked LSN</th>
                <th className="num">Local LSN</th>
                <th className="num">Lag</th>
                <th>Last ack</th>
                <th>Status</th>
              </tr>
            </thead>
            <tbody>
              {SITES.map((s) => (
                <tr key={s.siteId}>
                  <td className="mono">{s.siteId}</td>
                  <td className="num mono">{s.ackedLsn}</td>
                  <td className="num mono">{s.localLsn}</td>
                  <td className="num mono">{s.localLsn - s.ackedLsn}</td>
                  <td className="mono">{new Date(s.lastAckMs).toLocaleString()}</td>
                  <td>
                    <Pill tone={s.localLsn - s.ackedLsn === 0 ? 'ok' : 'warn'}>
                      {s.localLsn - s.ackedLsn === 0 ? 'in sync' : 'catching up'}
                    </Pill>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}
    </div>
  );
}
