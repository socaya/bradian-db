// Security & Audit — access grants, queries, exports, policy revisions.

import { Pill } from '@/components/Pill';
import { EmptyState } from '@/components/States';

interface Principal {
  id: string;
  role: 'Administrator' | 'Analyst' | 'System';
  scope: string[];
}
const PRINCIPALS: Principal[] = [];

interface AuditEntry {
  ts: string;
  principal: string;
  action: string;
  target: string;
  note?: string;
}
const AUDIT: AuditEntry[] = [];

export function SecurityPage() {
  return (
    <div className="stack">
      <div>
        <h1>Security &amp; Audit</h1>
        <p className="page-sub">
          Principals, roles, OU scopes and the append-only audit log for queries, exports
          and administrative actions.
        </p>
      </div>

      <section className="panel">
        <div className="panel-head">
          <h2>Principals</h2>
          <Pill tone="neutral">{PRINCIPALS.length}</Pill>
        </div>
        {PRINCIPALS.length === 0 ? (
          <div className="panel-body">
            <EmptyState
              title="No principals loaded"
              description="Register tokens through the admin CLI; the TokenStore hashes them with BLAKE3 before storage."
            />
          </div>
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>ID</th>
                <th>Role</th>
                <th>OU scope</th>
              </tr>
            </thead>
            <tbody>
              {PRINCIPALS.map((p) => (
                <tr key={p.id}>
                  <td>{p.id}</td>
                  <td>{p.role}</td>
                  <td className="mono">{p.scope.join(', ') || 'unrestricted'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>

      <section className="panel">
        <div className="panel-head">
          <h2>Audit log</h2>
          <Pill tone="neutral">last 100</Pill>
        </div>
        {AUDIT.length === 0 ? (
          <div className="panel-body">
            <EmptyState
              title="No audit events recorded"
              description="The AuditLog writes JSON Lines; the live-stream endpoint is next."
            />
          </div>
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>Timestamp</th>
                <th>Principal</th>
                <th>Action</th>
                <th>Target</th>
                <th>Note</th>
              </tr>
            </thead>
            <tbody>
              {AUDIT.map((a, i) => (
                <tr key={i}>
                  <td className="mono">{a.ts}</td>
                  <td>{a.principal}</td>
                  <td>{a.action}</td>
                  <td className="mono">{a.target}</td>
                  <td style={{ color: 'var(--text-secondary)' }}>{a.note ?? ''}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>
    </div>
  );
}
