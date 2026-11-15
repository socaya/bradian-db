// Data Products — manifest, measures, lineage, SLA, consumer list.

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { api } from '@/api/client';
import { EmptyState, FailedState, LoadingLine } from '@/components/States';
import { Pill } from '@/components/Pill';
import type { DataProduct } from '@/api/types';

const SAMPLE_PRODUCT: DataProduct = {
  id: 'dp_anc_monthly',
  owner: 'ncd-team',
  grain: ['ou', 'period'],
  measures: ['sum_value'],
  description: 'ANC monthly aggregate',
  consumers: [],
  freshness_contract: { max_staleness_seconds: 3600, consistency: 'EDGE_STABLE' },
};

export function DataProductsPage() {
  const list = useQuery({ queryKey: ['data-products'], queryFn: api.listDataProducts });
  const [selected, setSelected] = useState<DataProduct | null>(null);
  const qc = useQueryClient();
  const register = useMutation({
    mutationFn: (p: DataProduct) => api.registerDataProduct(p),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['data-products'] }),
  });

  if (list.isLoading) return <LoadingLine label="Reading catalogue…" />;
  if (list.error) return <FailedState message={(list.error as Error).message} retry={() => list.refetch()} />;

  return (
    <div className="stack">
      <div>
        <h1>Data Products</h1>
        <p className="page-sub">
          Reusable analytical outputs with declared contracts: grain, measures, SLA and
          the consumers that depend on them.
        </p>
      </div>

      <div className="btn-row" style={{ justifyContent: 'flex-end' }}>
        <button
          className="btn primary"
          onClick={() => register.mutate(SAMPLE_PRODUCT)}
          disabled={register.isPending}
        >
          Register sample product
        </button>
      </div>

      {(list.data ?? []).length === 0 ? (
        <EmptyState
          title="No data products yet"
          description="A product declares what the engine serves, who owns it, and the freshness contract consumers can rely on."
          action={
            <button
              className="btn primary"
              onClick={() => register.mutate(SAMPLE_PRODUCT)}
            >
              Register the ANC sample
            </button>
          }
        />
      ) : (
        <div className="two-col">
          <section className="panel">
            <div className="panel-head">
              <h2>Products</h2>
              <Pill tone="neutral">{list.data?.length}</Pill>
            </div>
            <table className="table">
              <thead>
                <tr>
                  <th>ID</th>
                  <th>Owner</th>
                  <th>Grain</th>
                  <th>Measures</th>
                  <th>SLA</th>
                </tr>
              </thead>
              <tbody>
                {(list.data ?? []).map((p) => (
                  <tr
                    key={p.id}
                    onClick={() => setSelected(p)}
                    style={{ cursor: 'pointer' }}
                  >
                    <td className="mono">{p.id}</td>
                    <td>{p.owner}</td>
                    <td className="mono">{p.grain.join(', ')}</td>
                    <td className="mono">{p.measures.join(', ')}</td>
                    <td className="mono">
                      {p.freshness_contract.max_staleness_seconds}s
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </section>

          <aside className="panel">
            <div className="panel-head">
              <h2>Manifest</h2>
            </div>
            <div className="panel-body">
              {selected ? (
                <dl className="kv">
                  <dt>ID</dt>
                  <dd className="mono">{selected.id}</dd>
                  <dt>Owner</dt>
                  <dd>{selected.owner}</dd>
                  <dt>Description</dt>
                  <dd>{selected.description || '—'}</dd>
                  <dt>Consumers</dt>
                  <dd>{selected.consumers.join(', ') || '—'}</dd>
                  <dt>Consistency</dt>
                  <dd className="mono">
                    {typeof selected.freshness_contract.consistency === 'string'
                      ? selected.freshness_contract.consistency
                      : JSON.stringify(selected.freshness_contract.consistency)}
                  </dd>
                </dl>
              ) : (
                <p style={{ color: 'var(--text-secondary)', fontSize: 'var(--fs-13)' }}>
                  Select a product to inspect its manifest.
                </p>
              )}
            </div>
          </aside>
        </div>
      )}
    </div>
  );
}
