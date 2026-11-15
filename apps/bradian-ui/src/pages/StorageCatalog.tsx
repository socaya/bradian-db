// Storage Catalog — table/projection versions, segments, compression, partitions.

import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { api } from '@/api/client';
import { Pill } from '@/components/Pill';
import { FailedState, LoadingLine } from '@/components/States';

export function StorageCatalogPage() {
  const health = useQuery({ queryKey: ['health'], queryFn: api.health });
  const catalog = useQuery({ queryKey: ['catalog'], queryFn: api.catalog });
  const [datasetFilter, setDatasetFilter] = useState('');
  const [partitionFilter, setPartitionFilter] = useState('');

  const freshness = useQuery({
    queryKey: ['freshness', datasetFilter, partitionFilter],
    queryFn: () => api.freshness(datasetFilter, partitionFilter),
    enabled: datasetFilter.length > 0 && partitionFilter.length > 0,
    retry: false,
  });

  if (catalog.isLoading) return <LoadingLine label="Reading catalog…" />;

  return (
    <div className="stack">
      <div>
        <h1>Storage Catalog</h1>
        <p className="page-sub">
          Columnar segments per partition, codec choices, compression ratios and visible
          generations. Every row references immutable `.derc` files on disk.
        </p>
      </div>

      <section className="tile-grid">
        <Tile label="Catalog revision" value={catalog.data?.revision.slice(0, 12)} />
        <Tile label="Partitions open" value={health.data?.partitions_open ?? '—'} />
        <Tile
          label="Format"
          value={<span className="mono">.derc v1</span>}
          sub="zstd + dict + plain"
        />
      </section>

      <section className="panel">
        <div className="panel-head">
          <h2>Inspect partition</h2>
        </div>
        <div className="panel-body form-grid">
          <label>
            <div className="label">Dataset ID</div>
            <input
              className="input"
              placeholder="dsAAAAAAAAA"
              value={datasetFilter}
              onChange={(e) => setDatasetFilter(e.target.value)}
            />
          </label>
          <label>
            <div className="label">Partition ID</div>
            <input
              className="input"
              placeholder="p1"
              value={partitionFilter}
              onChange={(e) => setPartitionFilter(e.target.value)}
            />
          </label>
        </div>
      </section>

      {freshness.error ? (
        <FailedState
          title="Partition not reachable"
          message={(freshness.error as Error).message}
          retry={() => freshness.refetch()}
        />
      ) : freshness.data ? (
        <section className="panel">
          <div className="panel-head">
            <h2>{freshness.data.dataset} · {freshness.data.partition}</h2>
            <Pill tone="ok">readable</Pill>
          </div>
          <div className="panel-body">
            <dl className="kv">
              <dt>Segments</dt>
              <dd>{freshness.data.segment_count}</dd>
              <dt>Data revision</dt>
              <dd className="mono">{freshness.data.data_revision}</dd>
              <dt>Watermark</dt>
              <dd className="mono">
                {freshness.data.watermark_ms === 0
                  ? '—'
                  : new Date(freshness.data.watermark_ms).toISOString()}
              </dd>
            </dl>
          </div>
        </section>
      ) : (
        <p style={{ color: 'var(--text-secondary)', fontSize: 'var(--fs-13)' }}>
          Enter a dataset + partition to inspect it.
        </p>
      )}
    </div>
  );
}

function Tile({ label, value, sub }: { label: string; value: React.ReactNode; sub?: string }) {
  return (
    <div className="tile">
      <div className="label">{label}</div>
      <div className="value">{value}</div>
      {sub && <div className="sub">{sub}</div>}
    </div>
  );
}
