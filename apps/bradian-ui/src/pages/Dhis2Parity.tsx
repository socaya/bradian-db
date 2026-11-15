// DHIS2 Parity — version support and analytical result diffs.

import { Pill } from '@/components/Pill';

interface ParityRow {
  area: string;
  status: 'done' | 'partial' | 'defer';
  tests: number;
  note: string;
}

const MATRIX: ParityRow[] = [
  { area: 'Expression grammar (operators + literals)', status: 'done',    tests: 24, note: 'Full operator matrix incl. logicals and short-circuit' },
  { area: 'Whitelisted functions (if, coalesce, log...)', status: 'done',  tests: 9,  note: '13 functions implemented' },
  { area: 'd2: namespace', status: 'done', tests: 7, note: '14 d2: functions' },
  { area: 'Aggregate roll-up (SUM/AVG/AVG_SUM_OU/COUNT/MIN/MAX)', status: 'done', tests: 7, note: 'Matches DHIS2 reference matrix' },
  { area: 'STDDEV / VARIANCE / LAST / LAST_IN_PERIOD',         status: 'done', tests: 3, note: 'Welford + max-timestamp' },
  { area: 'Period offsets',                                    status: 'done', tests: 1, note: '#{DE}.periodOffset(N)' },
  { area: 'Reporting-rate variants',                           status: 'done', tests: 5, note: '5 variants, zero-expected -> null' },
  { area: 'Event program indicator',                           status: 'done', tests: 5, note: 'AnalyticsType::Event' },
  { area: 'Tracker enrollment program indicator',              status: 'done', tests: 6, note: 'distinct TEI, D{stage.de}' },
  { area: 'Relationship analytics',                            status: 'done', tests: 3, note: 'd2:relationshipCount' },
  { area: 'Program indicator CUSTOM aggregation',              status: 'done', tests: 5, note: 'incl. Spearman / Kendall' },
  { area: 'Classic indicators (num/denom/factor)',             status: 'done', tests: 10, note: 'Null + divide-by-zero semantics' },
  { area: 'Program rule execution',                            status: 'defer', tests: 0, note: 'Capture-layer responsibility' },
  { area: 'Live /api/metadata HTTP adapter',                   status: 'done', tests: 1, note: 'Basic / bearer auth' },
];

export function Dhis2ParityPage() {
  const done = MATRIX.filter((m) => m.status === 'done').length;
  const partial = MATRIX.filter((m) => m.status === 'partial').length;
  const defer = MATRIX.filter((m) => m.status === 'defer').length;
  const totalTests = MATRIX.reduce((a, r) => a + r.tests, 0);
  return (
    <div className="stack">
      <div>
        <h1>DHIS2 Parity</h1>
        <p className="page-sub">
          Which DHIS2 analytical capabilities the engine covers, with test counts backing
          each claim. Any <em>done</em> row carries at least one reference fixture; any
          <em> defer</em> row documents the remaining work.
        </p>
      </div>

      <section className="tile-grid">
        <Tile label="Done" value={done} sub={`${totalTests} tests passing`} />
        <Tile label="Partial" value={partial} />
        <Tile label="Deferred" value={defer} />
      </section>

      <section className="panel">
        <table className="table">
          <thead>
            <tr>
              <th>Area</th>
              <th>Status</th>
              <th className="num">Tests</th>
              <th>Note</th>
            </tr>
          </thead>
          <tbody>
            {MATRIX.map((row) => (
              <tr key={row.area}>
                <td>{row.area}</td>
                <td>
                  <Pill tone={row.status === 'done' ? 'ok' : row.status === 'partial' ? 'warn' : 'neutral'}>
                    {row.status}
                  </Pill>
                </td>
                <td className="num">{row.tests}</td>
                <td style={{ color: 'var(--text-secondary)' }}>{row.note}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>
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
