// Query Workbench — query definition, EXPLAIN, actual profile, results.

import { useMutation } from '@tanstack/react-query';
import { useState } from 'react';
import { api, ApiError } from '@/api/client';
import { Pill } from '@/components/Pill';
import { FailedState } from '@/components/States';
import type { QueryRequest, QueryResponse, ExplainPlan, ValueJson } from '@/api/types';

const SAMPLE = `{
  "kind": "aggregate",
  "dataset": "dsAAAAAAAAA",
  "partition": "p1",
  "measures": [{ "op": "sum", "column": "value" }],
  "group_by": ["ou", "period"],
  "period": { "range": { "period_type": "MONTHLY", "start": "202601", "end": "202612" } },
  "filters": [],
  "order_by": []
}`;

export function QueryWorkbenchPage() {
  const [source, setSource] = useState(SAMPLE);
  const [parseError, setParseError] = useState<string | null>(null);

  const run = useMutation({
    mutationFn: (q: QueryRequest) => api.runQuery(q),
  });
  const explain = useMutation({
    mutationFn: (q: QueryRequest) => api.explainQuery(q),
  });

  const parse = (): QueryRequest | null => {
    setParseError(null);
    try {
      return JSON.parse(source) as QueryRequest;
    } catch (e) {
      setParseError(String(e));
      return null;
    }
  };

  const onRun = () => {
    const q = parse();
    if (q) run.mutate(q);
  };
  const onExplain = () => {
    const q = parse();
    if (q) explain.mutate(q);
  };

  return (
    <div className="stack">
      <div>
        <h1>Query Workbench</h1>
        <p className="page-sub">
          Author a semantic query in the DAP DSL. Run returns rows and provenance;
          Explain shows the chosen plan. Policies are enforced server-side.
        </p>
      </div>

      <section className="panel">
        <div className="panel-head">
          <h2>Query</h2>
          <div className="btn-row">
            <button className="btn" onClick={onExplain} disabled={explain.isPending}>
              {explain.isPending ? 'Explaining…' : 'Explain'}
            </button>
            <button className="btn primary" onClick={onRun} disabled={run.isPending}>
              {run.isPending ? 'Running…' : 'Run'}
            </button>
          </div>
        </div>
        <div className="panel-body">
          <textarea
            className="textarea"
            rows={14}
            spellCheck={false}
            value={source}
            onChange={(e) => setSource(e.target.value)}
          />
          {parseError && (
            <div className="state-line err" style={{ marginTop: 'var(--sp-3)' }}>
              <Pill tone="err">parse error</Pill>
              <span className="mono">{parseError}</span>
            </div>
          )}
        </div>
      </section>

      {explain.data && <ExplainPane plan={explain.data} />}
      {explain.error && (
        <FailedState message={apiMessage(explain.error)} retry={onExplain} />
      )}

      {run.data && <ResultPane result={run.data} />}
      {run.error && <FailedState message={apiMessage(run.error)} retry={onRun} />}
    </div>
  );
}

function apiMessage(e: unknown): string {
  if (e instanceof ApiError) return `${e.code}: ${e.message}`;
  if (e instanceof Error) return e.message;
  return String(e);
}

function ExplainPane({ plan }: { plan: ExplainPlan }) {
  return (
    <section className="panel">
      <div className="panel-head">
        <h2>Explain</h2>
        <span className="mono" style={{ fontSize: 'var(--fs-12)', color: 'var(--text-muted)' }}>
          metadata {plan.metadata_revision.slice(0, 12)}
        </span>
      </div>
      <div className="panel-body">
        <PlanNode node={plan.root} />
      </div>
    </section>
  );
}

function PlanNode({ node, depth = 0 }: { node: ExplainPlan['root']; depth?: number }) {
  return (
    <div style={{ marginLeft: depth * 16 }}>
      <div style={{ display: 'flex', gap: 'var(--sp-2)', alignItems: 'baseline' }}>
        <strong style={{ fontSize: 'var(--fs-13)' }}>{node.name}</strong>
        <span className="mono" style={{ color: 'var(--text-muted)', fontSize: 'var(--fs-12)' }}>
          {node.details}
        </span>
        {node.estimated_rows !== undefined && (
          <Pill tone="neutral">~{node.estimated_rows} rows</Pill>
        )}
      </div>
      {node.children.map((c, i) => (
        <PlanNode key={i} node={c} depth={depth + 1} />
      ))}
    </div>
  );
}

function ResultPane({ result }: { result: QueryResponse }) {
  return (
    <section className="panel">
      <div className="panel-head">
        <h2>Result</h2>
        <div className="btn-row">
          <Pill tone={result.provenance.is_exact ? 'ok' : 'warn'}>
            {result.provenance.is_exact ? 'exact' : 'approximate'}
          </Pill>
          <span className="mono" style={{ fontSize: 'var(--fs-12)' }}>
            {result.metrics.scanned_rows} scanned · {result.metrics.output_rows} returned
          </span>
        </div>
      </div>
      <div className="scroll" style={{ borderRadius: 0, borderLeft: 0, borderRight: 0 }}>
        <table className="table">
          <thead>
            <tr>
              {result.columns.map((c) => (
                <th key={c}>{c}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {result.rows.map((row, i) => (
              <tr key={i}>
                {row.map((cell, j) => (
                  <td key={j} className="mono">
                    {renderCell(cell)}
                  </td>
                ))}
              </tr>
            ))}
            {result.rows.length === 0 && (
              <tr className="row-empty">
                <td colSpan={result.columns.length}>no rows</td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
      <div className="panel-footer">
        <span>
          data revision {result.provenance.data_revision} · watermark{' '}
          {result.provenance.watermark_ms > 0
            ? new Date(result.provenance.watermark_ms).toISOString()
            : '—'}
        </span>
      </div>
    </section>
  );
}

function renderCell(v: ValueJson): string {
  const key = Object.keys(v)[0];
  const val = (v as Record<string, unknown>)[key];
  if (val === null) return 'NULL';
  return String(val);
}
