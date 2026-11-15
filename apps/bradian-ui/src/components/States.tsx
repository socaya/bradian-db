// State primitives: loading, empty, offline, degraded, partial, permission-denied, stale,
// conflicting, failed. DAP every screen handle these deliberately.

import type { ReactNode } from 'react';
import { Pill } from './Pill';

export function LoadingLine({ label = 'Loading…' }: { label?: string }) {
  return (
    <div className="state-line">
      <span className="spinner" aria-hidden />
      <span>{label}</span>
    </div>
  );
}

export function EmptyState({
  title,
  description,
  action,
}: {
  title: string;
  description?: string;
  action?: ReactNode;
}) {
  return (
    <div className="panel">
      <div className="panel-body" style={{ textAlign: 'center' }}>
        <h3 style={{ marginBottom: 6 }}>{title}</h3>
        {description && (
          <p style={{ color: 'var(--text-secondary)', fontSize: 'var(--fs-13)' }}>
            {description}
          </p>
        )}
        {action && <div style={{ marginTop: 'var(--sp-3)' }}>{action}</div>}
      </div>
    </div>
  );
}

export function FailedState({
  title = 'The engine refused the request.',
  message,
  retry,
}: {
  title?: string;
  message: string;
  retry?: () => void;
}) {
  return (
    <div className="state-line err">
      <div style={{ flex: 1 }}>
        <div style={{ fontWeight: 600 }}>{title}</div>
        <div className="mono" style={{ marginTop: 2 }}>
          {message}
        </div>
      </div>
      {retry && (
        <button className="btn sm" onClick={retry}>
          Retry
        </button>
      )}
    </div>
  );
}

export function PermissionDenied({ what }: { what: string }) {
  return (
    <div className="state-line warn">
      <Pill tone="warn">permission denied</Pill>
      <span>You do not have access to {what}.</span>
    </div>
  );
}

export function PartialState({
  partial,
  missing,
}: {
  partial: string[];
  missing: string[];
}) {
  if (partial.length === 0 && missing.length === 0) return null;
  return (
    <div className="state-line warn">
      <Pill tone="warn">partial coverage</Pill>
      <div className="mono" style={{ fontSize: 'var(--fs-12)' }}>
        {partial.length > 0 && <>partial: {partial.join(', ')} · </>}
        {missing.length > 0 && <>missing: {missing.join(', ')}</>}
      </div>
    </div>
  );
}

export function OfflineBanner() {
  return (
    <div className="state-line warn">
      <Pill tone="warn">offline</Pill>
      <span>
        The engine looks offline. Displaying the last snapshot that reached this browser —
        provenance may be stale.
      </span>
    </div>
  );
}

export function StaleBanner({ ageMs }: { ageMs: number }) {
  const minutes = Math.round(ageMs / 60_000);
  return (
    <div className="state-line warn">
      <Pill tone="warn">stale</Pill>
      <span>Data last refreshed {minutes} minute{minutes === 1 ? '' : 's'} ago.</span>
    </div>
  );
}
