// Backups & Recovery — snapshots, log positions, restore checks.

import { Pill } from '@/components/Pill';
import { EmptyState } from '@/components/States';

interface Snapshot {
  id: string;
  dataset: string;
  partition: string;
  dataRevision: number;
  segments: number;
  rows: number;
  exportedAt: string;
}
const SNAPSHOTS: Snapshot[] = [];

export function BackupsPage() {
  return (
    <div className="stack">
      <div>
        <h1>Backups &amp; Recovery</h1>
        <p className="page-sub">
          Immutable snapshots of partitions, their data revisions and restore targets.
          Snapshots are created with <code className="mono">dap-admin export-snapshot</code>
          and restored with <code className="mono">dap-admin restore-snapshot</code>.
        </p>
      </div>

      {SNAPSHOTS.length === 0 ? (
        <EmptyState
          title="No snapshots indexed"
          description="Snapshots live on disk under a directory the operator chooses; the indexer that enumerates them inside the admin app is the next deliverable."
          action={<code className="mono">dap-admin export-snapshot --root /data --dataset D --partition P --out /snapshots/D-P-1</code>}
        />
      ) : (
        <section className="panel">
          <table className="table">
            <thead>
              <tr>
                <th>ID</th>
                <th>Dataset</th>
                <th>Partition</th>
                <th className="num">Rev</th>
                <th className="num">Segments</th>
                <th className="num">Rows</th>
                <th>Exported</th>
                <th>Status</th>
              </tr>
            </thead>
            <tbody>
              {SNAPSHOTS.map((s) => (
                <tr key={s.id}>
                  <td className="mono">{s.id}</td>
                  <td>{s.dataset}</td>
                  <td className="mono">{s.partition}</td>
                  <td className="num mono">{s.dataRevision}</td>
                  <td className="num">{s.segments}</td>
                  <td className="num">{s.rows}</td>
                  <td className="mono">{s.exportedAt}</td>
                  <td>
                    <Pill tone="ok">valid</Pill>
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
