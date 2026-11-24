# ADR 0320 — HistorySqlEngine samples DurableTypedStore memory

## Status

Accepted.

## Context

G1-MEMORY qualification tail: `HistorySqlEngine` was
sampling only the catalog into `WorkspaceUsage` before each
statement. The retained-history keys / versions / bytes
held in its own `DurableTypedStore` were excluded from the
workspace budget. ADR 0171 added
`WorkspaceUsage::with_durable_typed(..)`; this ADR wires
it up.

## Consequences

- Workspace budget rejections now account for the history
  engine's durable-typed store on every statement entry.
- Zero API change; one-line call-site addition.
- Regression: `history_admission_sql` 2/2 still green.
