// Materialized Views — dependencies, version, watermark, staleness.

import { EmptyState } from '@/components/States';
import { Pill } from '@/components/Pill';

export function MaterializedViewsPage() {
  // The engine exposes materialised views through dap-views at the library level. A
  // dedicated /views listing endpoint is scheduled next; today operators discover views
  // via data products.
  return (
    <div className="stack">
      <div>
        <h1>Materialized Views</h1>
        <p className="page-sub">
          Cached projections of measures across a grid. Each view carries dependencies,
          data + metadata revision, upstream watermark and a staleness classifier.
        </p>
      </div>

      <section className="panel">
        <div className="panel-head">
          <h2>Known views</h2>
          <div className="btn-row">
            <Pill tone="info">live listing pending</Pill>
          </div>
        </div>
        <div className="panel-body">
          <EmptyState
            title="Views are registered server-side"
            description="Call `materialize_aggregate_indicator` / `materialize_event_program_indicator` from the admin CLI or an ingestion job; a listing endpoint under /api/analytics/v1/views ships next iteration."
          />
        </div>
      </section>
    </div>
  );
}
