import type { ReactNode } from 'react';

type Tone = 'ok' | 'warn' | 'err' | 'info' | 'neutral';

export function Pill({ tone = 'neutral', children }: { tone?: Tone; children: ReactNode }) {
  return <span className={`pill ${tone}`}>{children}</span>;
}
