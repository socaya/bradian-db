// Distributed Nodes — site/worker health, resources, fragments, availability.

import { Pill } from '@/components/Pill';
import { EmptyState } from '@/components/States';

interface NodeRow {
  id: string;
  location: string;
  role: 'coordinator' | 'worker' | 'edge';
  status: 'ok' | 'warn' | 'err';
  datasets: number;
  fragmentsPerMin: number;
  lagMs: number;
}

const NODES: NodeRow[] = [];

export function DistributedNodesPage() {
  return (
    <div className="stack">
      <div>
        <h1>Distributed Nodes</h1>
        <p className="page-sub">
          Workers and edge sites participating in federated analytics. Each row shows the
          declared capabilities, current fragment throughput and reconciliation lag.
        </p>
      </div>

      {NODES.length === 0 ? (
        <EmptyState
          title="No workers registered"
          description="The coordinator is reachable but no worker has joined. Register a worker through dap-admin or wire an InProcessWorker from your own crate."
        />
      ) : (
        <section className="panel">
          <table className="table">
            <thead>
              <tr>
                <th>ID</th>
                <th>Location</th>
                <th>Role</th>
                <th>Status</th>
                <th className="num">Datasets</th>
                <th className="num">Fragments/min</th>
                <th className="num">Lag (ms)</th>
              </tr>
            </thead>
            <tbody>
              {NODES.map((n) => (
                <tr key={n.id}>
                  <td className="mono">{n.id}</td>
                  <td>{n.location}</td>
                  <td>{n.role}</td>
                  <td>
                    <Pill tone={n.status}>{n.status}</Pill>
                  </td>
                  <td className="num">{n.datasets}</td>
                  <td className="num">{n.fragmentsPerMin}</td>
                  <td className="num">{n.lagMs}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}
    </div>
  );
}
