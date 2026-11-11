// Settings — quotas, profiles, feature flags, retention.

import { useState } from 'react';
import { Pill } from '@/components/Pill';

interface FeatureFlag {
  key: string;
  label: string;
  description: string;
  enabled: boolean;
}

const DEFAULTS: FeatureFlag[] = [
  {
    key: 'zone_map_pruning',
    label: 'Zone-map segment pruning',
    description: 'Skip segments whose per-column min/max cannot satisfy the predicate.',
    enabled: true,
  },
  {
    key: 'parallel_aggregate',
    label: 'Parallel filter + aggregate',
    description: 'Shard batches across the rayon worker pool before merging partials.',
    enabled: true,
  },
  {
    key: 'spill_to_disk',
    label: 'Hash-aggregate disk spill',
    description: 'Flush the hash table to a temp file when memory exceeds the budget.',
    enabled: true,
  },
  {
    key: 'residency_policy_enforcement',
    label: 'Residency policy enforcement',
    description: 'Refuse federated fragments that exceed a worker\'s declared egress class.',
    enabled: true,
  },
];

interface Quota {
  key: string;
  label: string;
  value: number;
  unit: string;
}

const QUOTAS: Quota[] = [
  { key: 'memory_mb',    label: 'Per-query memory',             value: 1024, unit: 'MB' },
  { key: 'scan_rows',    label: 'Per-query scanned rows',        value: 10_000_000, unit: 'rows' },
  { key: 'export_rows',  label: 'Max export rows (bounded)',     value: 500_000,    unit: 'rows' },
  { key: 'job_ttl_min',  label: 'Async job retention',           value: 60, unit: 'minutes' },
];

export function SettingsPage() {
  const [flags, setFlags] = useState(DEFAULTS);
  const [quotas, setQuotas] = useState(QUOTAS);
  const dirty = flags.some((f, i) => f.enabled !== DEFAULTS[i].enabled)
    || quotas.some((q, i) => q.value !== QUOTAS[i].value);
  return (
    <div className="stack">
      <div>
        <h1>Settings</h1>
        <p className="page-sub">
          Query quotas, workload profiles, feature flags and retention windows. Changes
          are versioned and validated before being applied.
        </p>
      </div>

      <section className="panel">
        <div className="panel-head">
          <h2>Feature flags</h2>
          <Pill tone="info">{flags.filter((f) => f.enabled).length} enabled</Pill>
        </div>
        <table className="table">
          <thead>
            <tr>
              <th>Flag</th>
              <th>Description</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {flags.map((flag, i) => (
              <tr key={flag.key}>
                <td className="mono">{flag.key}</td>
                <td style={{ color: 'var(--text-secondary)' }}>{flag.description}</td>
                <td style={{ textAlign: 'right' }}>
                  <label
                    style={{
                      display: 'inline-flex',
                      gap: 'var(--sp-2)',
                      alignItems: 'center',
                      fontSize: 'var(--fs-12)',
                    }}
                  >
                    <input
                      type="checkbox"
                      checked={flag.enabled}
                      onChange={(e) =>
                        setFlags((prev) => {
                          const next = [...prev];
                          next[i] = { ...next[i], enabled: e.target.checked };
                          return next;
                        })
                      }
                    />
                    {flag.enabled ? 'enabled' : 'disabled'}
                  </label>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>

      <section className="panel">
        <div className="panel-head">
          <h2>Quotas</h2>
        </div>
        <table className="table">
          <thead>
            <tr>
              <th>Key</th>
              <th>Description</th>
              <th className="num">Value</th>
              <th>Unit</th>
            </tr>
          </thead>
          <tbody>
            {quotas.map((q, i) => (
              <tr key={q.key}>
                <td className="mono">{q.key}</td>
                <td>{q.label}</td>
                <td className="num">
                  <input
                    className="input mono"
                    style={{ width: 120, textAlign: 'right' }}
                    type="number"
                    value={q.value}
                    onChange={(e) =>
                      setQuotas((prev) => {
                        const next = [...prev];
                        next[i] = { ...next[i], value: Number(e.target.value) };
                        return next;
                      })
                    }
                  />
                </td>
                <td className="mono">{q.unit}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>

      <div className="btn-row" style={{ justifyContent: 'flex-end' }}>
        <button className="btn" disabled={!dirty}>
          Discard
        </button>
        <button className="btn primary" disabled={!dirty}>
          Validate &amp; apply
        </button>
      </div>
    </div>
  );
}
