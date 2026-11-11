// Data Sources — source type, checkpoint, failures, schema.

import { useState } from 'react';
import { Pill } from '@/components/Pill';
import { EmptyState } from '@/components/States';

interface Source {
  id: string;
  kind: 'DHIS2 /api/metadata' | 'CSV file' | 'JSON Lines' | 'SQL CDC';
  status: 'ok' | 'warn' | 'err' | 'paused';
  lastCheckpoint: string;
  failures: number;
  schema: string;
}

// The engine persists these in a side table (OPS-004 scope); meanwhile the operator
// drives registration through dap-admin CLI and the UI lists what the server reports.
const STUB_SOURCES: Source[] = [];

export function DataSourcesPage() {
  const [filter, setFilter] = useState('');
  const sources = STUB_SOURCES.filter((s) =>
    s.id.toLowerCase().includes(filter.toLowerCase()),
  );
  return (
    <div className="stack">
      <div>
        <h1>Data Sources</h1>
        <p className="page-sub">
          Registered inputs feeding the analytical fabric. Each source states its latest
          checkpoint, failure count and declared schema. Use the admin CLI to register new
          sources — the engine refuses unknown schemas.
        </p>
      </div>

      <div className="btn-row" style={{ justifyContent: 'space-between' }}>
        <input
          className="input"
          placeholder="Filter sources"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          style={{ maxWidth: 320 }}
        />
        <a className="btn" href="/sources/new">
          Register source
        </a>
      </div>

      {sources.length === 0 ? (
        <EmptyState
          title="No data sources registered"
          description="Register a CSV file, JSONL stream, SQL CDC source or a DHIS2 metadata adapter through the admin CLI, then they will appear here."
          action={
            <code className="mono">
              dap-admin load-dhis2-metadata --file bundle.json
            </code>
          }
        />
      ) : (
        <div className="panel">
          <table className="table">
            <thead>
              <tr>
                <th>ID</th>
                <th>Kind</th>
                <th>Status</th>
                <th>Last checkpoint</th>
                <th className="num">Failures</th>
                <th>Schema</th>
              </tr>
            </thead>
            <tbody>
              {sources.map((s) => (
                <tr key={s.id}>
                  <td className="mono">{s.id}</td>
                  <td>{s.kind}</td>
                  <td>
                    <Pill tone={s.status === 'paused' ? 'neutral' : s.status}>
                      {s.status}
                    </Pill>
                  </td>
                  <td className="mono">{s.lastCheckpoint}</td>
                  <td className="num">{s.failures}</td>
                  <td className="mono">{s.schema}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
