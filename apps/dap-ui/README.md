# dap-admin-ui

Full-screen admin + analytics workbench for DAP (spec DER-ANL-PLAT-001 §13).

## Stack

- React 18 + TypeScript
- React Router 6
- TanStack Query (`@tanstack/react-query`)
- Vite 5 bundler
- Zero external design-system; `src/design/*.css` carries the spec-compliant tokens

## Install and run

```bash
cd apps/dap-admin-ui
npm install
npm run dev           # dev server on :5173, proxies /api -> :8080
npm run build         # emits dist/ for static serving
npm run typecheck
```

The dev server proxies `/api` to the Rust engine running on `127.0.0.1:8080`. To serve
the production bundle, point any static file server at `dist/` (or mount it behind the
`dap-serving` crate).

## Pages (spec §13)

| Route | Page |
|---|---|
| `/` | Platform Overview |
| `/sources` | Data Sources |
| `/storage` | Storage Catalog |
| `/workbench` | Query Workbench (EXPLAIN + run + results) |
| `/history` | Query History |
| `/views` | Materialized Views |
| `/products` | Data Products |
| `/nodes` | Distributed Nodes |
| `/reconciliation` | Reconciliation |
| `/dhis2-parity` | DHIS2 Parity matrix |
| `/security` | Security &amp; Audit |
| `/backups` | Backups &amp; Recovery |
| `/benchmarks` | Benchmarks |
| `/settings` | Settings (quotas, flags) |

## Design rules (spec §13)

- Lists, details and edit forms are separate full-page routes. No master-detail splits
  inside a single route.
- Context drawers (the right column in two-column pages) are strictly for inspection and
  diagnostics — never for editable forms.
- Three surface planes only (page / panel / muted), one accent colour family for state,
  no gradients, no decorative badges.
- Every page explicitly handles loading / empty / offline / degraded / partial /
  permission-denied / stale / conflicting / failed states via `src/components/States.tsx`.
- Numeric columns use tabular numerics and right-align; identifier columns use a
  monospace font.

## Wiring

- `src/api/types.ts` mirrors the Rust `dap-serving` structs. Keep in sync by hand (a
  codegen pass against the `OpenAPI` schema is a later deliverable).
- `src/api/client.ts` is a thin `fetch` wrapper that surfaces typed `ApiError`s so the
  UI can map HTTP 401 / 403 / 404 / 410 / 413 / 500 to the right state.
