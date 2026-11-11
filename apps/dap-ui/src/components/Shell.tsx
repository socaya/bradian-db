import { NavLink, Outlet, useLocation } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { api } from '@/api/client';
import { Pill } from './Pill';
import '@/design/layout.css';

const NAV: { to: string; label: string; hint: string; section: string }[] = [
  { section: 'Operate',   to: '/',                 label: 'Overview',          hint: 'g o' },
  { section: 'Operate',   to: '/sources',          label: 'Data Sources',      hint: 'g s' },
  { section: 'Operate',   to: '/storage',          label: 'Storage Catalog',   hint: 'g c' },
  { section: 'Operate',   to: '/nodes',            label: 'Distributed Nodes', hint: 'g n' },
  { section: 'Operate',   to: '/reconciliation',   label: 'Reconciliation',    hint: 'g r' },

  { section: 'Analyse',   to: '/workbench',        label: 'Query Workbench',   hint: 'g q' },
  { section: 'Analyse',   to: '/history',          label: 'Query History',     hint: 'g h' },
  { section: 'Analyse',   to: '/views',            label: 'Materialized Views',hint: 'g m' },
  { section: 'Analyse',   to: '/products',         label: 'Data Products',     hint: 'g p' },
  { section: 'Analyse',   to: '/dhis2-parity',     label: 'DHIS2 Parity',      hint: 'g d' },

  { section: 'Governance',to: '/security',         label: 'Security & Audit',  hint: 'g u' },
  { section: 'Governance',to: '/backups',          label: 'Backups & Recovery',hint: 'g b' },
  { section: 'Governance',to: '/benchmarks',       label: 'Benchmarks',        hint: 'g l' },
  { section: 'Governance',to: '/settings',         label: 'Settings',          hint: 'g ,' },
];

function CrumbTrail() {
  const { pathname } = useLocation();
  const match = NAV.find((n) => n.to === pathname) ?? NAV[0];
  return (
    <div className="crumbs">
      <span>DAP</span>
      <span className="sep">/</span>
      <span>{match.section}</span>
      <span className="sep">/</span>
      <span style={{ color: 'var(--text-primary)' }}>{match.label}</span>
    </div>
  );
}

function StatusLine() {
  const { data, isLoading, error } = useQuery({
    queryKey: ['health'],
    queryFn: api.health,
    refetchInterval: 15_000,
  });
  if (error) {
    return (
      <div className="status">
        <Pill tone="err">engine unreachable</Pill>
      </div>
    );
  }
  if (isLoading || !data) {
    return (
      <div className="status">
        <span className="spinner" aria-hidden />
        <span>connecting…</span>
      </div>
    );
  }
  return (
    <div className="status">
      <Pill tone="ok">engine healthy</Pill>
      <span>partitions {data.partitions_open}</span>
      <span>queries {data.query_history_size}</span>
      <span className="mono">cat {data.catalog_revision.slice(0, 8)}</span>
    </div>
  );
}

export function Shell() {
  const sections = Array.from(new Set(NAV.map((n) => n.section)));
  return (
    <div className="shell">
      <aside className="nav">
        <div className="brand">
          <span className="mark">DAP</span>
          <span>Analytics Platform</span>
        </div>
        {sections.map((section) => (
          <div key={section}>
            <div className="section-label">{section}</div>
            <div className="nav-group">
              {NAV.filter((n) => n.section === section).map((n) => (
                <NavLink
                  key={n.to}
                  to={n.to}
                  end={n.to === '/'}
                  className={({ isActive }) => (isActive ? 'active' : '')}
                >
                  <span>{n.label}</span>
                  <span className="hint">{n.hint}</span>
                </NavLink>
              ))}
            </div>
          </div>
        ))}
        <div className="footer">
          <div>spec DAP</div>
          <div>v0.1.0</div>
        </div>
      </aside>
      <main className="main">
        <div className="topbar">
          <CrumbTrail />
          <StatusLine />
        </div>
        <section className="content">
          <Outlet />
        </section>
      </main>
    </div>
  );
}
