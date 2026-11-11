// Thin wrapper over `fetch`. Returns typed envelopes; surfaces HTTP errors verbatim so
// the UI can distinguish 401 (sign in), 403 (permission), 404 (not found), 413 (budget),
// 410 (cancelled), 500 (internal) and show the right state.

import type {
  CatalogSummary,
  DataProduct,
  EngineHealth,
  ExplainPlan,
  FreshnessResponse,
  JobRecord,
  QueryRequest,
  QueryResponse,
} from './types';

export class ApiError extends Error {
  constructor(public status: number, public code: string, message: string) {
    super(message);
  }
}

async function request<T>(url: string, init?: RequestInit): Promise<T> {
  const res = await fetch(url, {
    headers: { 'content-type': 'application/json', ...(init?.headers ?? {}) },
    ...init,
  });
  if (!res.ok) {
    let code = 'http_' + res.status;
    let message = res.statusText;
    try {
      const body = await res.json();
      code = body.code ?? code;
      message = body.message ?? message;
    } catch {
      // body not JSON; fall back to status text
    }
    throw new ApiError(res.status, code, message);
  }
  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
}

export const api = {
  health: () => request<EngineHealth>('/api/analytics/v1/engine/health'),

  catalog: () => request<CatalogSummary>('/api/analytics/v1/catalog'),

  freshness: (dataset: string, partition: string) =>
    request<FreshnessResponse>('/api/analytics/v1/freshness', {
      method: 'POST',
      body: JSON.stringify({ dataset, partition }),
    }),

  runQuery: (request_: QueryRequest) =>
    request<QueryResponse>('/api/analytics/v1/query', {
      method: 'POST',
      body: JSON.stringify({ request: request_ }),
    }),

  explainQuery: (request_: QueryRequest) =>
    request<ExplainPlan>('/api/analytics/v1/query/explain', {
      method: 'POST',
      body: JSON.stringify({ request: request_ }),
    }),

  drilldown: (request_: QueryRequest, drillFilters: unknown[], limit = 1000) =>
    request<QueryResponse>('/api/analytics/v1/drilldown', {
      method: 'POST',
      body: JSON.stringify({ request: request_, drill_filters: drillFilters, limit }),
    }),

  listDataProducts: () => request<DataProduct[]>('/api/analytics/v1/data-products'),

  registerDataProduct: (product: DataProduct) =>
    request<DataProduct>('/api/analytics/v1/data-products', {
      method: 'POST',
      body: JSON.stringify(product),
    }),

  listJobs: () => request<JobRecord[]>('/api/analytics/v1/jobs'),

  getJob: (id: string) => request<JobRecord>(`/api/analytics/v1/jobs/${encodeURIComponent(id)}`),

  cancelJob: (id: string) =>
    request<{ id: string; status: string }>(
      `/api/analytics/v1/jobs/${encodeURIComponent(id)}/cancel`,
      { method: 'POST' },
    ),

  submitAsyncQuery: (request_: QueryRequest, submittedBy?: string) =>
    request<JobRecord>('/api/analytics/v1/jobs', {
      method: 'POST',
      body: JSON.stringify({ request: request_, submitted_by: submittedBy }),
    }),
};
