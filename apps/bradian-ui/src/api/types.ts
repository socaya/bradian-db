// Shapes mirror the Rust serde structs in dap-serving / dap-core / dap-planner /
// dap-storage. Keep these aligned by hand; a codegen pass is a later iteration.

export type ConsistencyMode =
  | 'EDGE_LATEST'
  | 'EDGE_STABLE'
  | 'CENTRAL_ACKNOWLEDGED'
  | { AsOf: number };

export interface SiteCoverage {
  complete: string[];
  partial: string[];
  missing: string[];
}

export interface Provenance {
  data_revision: number;
  metadata_revision: string;
  consistency: ConsistencyMode;
  watermark_ms: number;
  site_coverage: SiteCoverage;
  is_exact: boolean;
  note?: string;
}

export interface RuntimeMetrics {
  scanned_rows: number;
  scanned_bytes: number;
  output_rows: number;
  peak_memory_bytes: number;
  spilled_bytes: number;
}

export type ValueJson =
  | { Null: null }
  | { Bool: boolean }
  | { I64: number }
  | { F64: number }
  | { Decimal128: { value: string; scale: number } }
  | { Str: string }
  | { Uid: string }
  | { Timestamp: number }
  | { Period: string };

export interface QueryResponse {
  columns: string[];
  rows: ValueJson[][];
  provenance: Provenance;
  metrics: RuntimeMetrics;
}

export interface QueryRequestAggregate {
  kind: 'aggregate';
  dataset: string;
  partition: string;
  measures: AggregationSpec[];
  group_by: string[];
  period?: PeriodSpec;
  org_unit?: OrgUnitSelector;
  filters: FilterSpec[];
  order_by: OrderSpec[];
  limit?: number;
}

export interface QueryRequestProgramIndicator {
  kind: 'program_indicator';
  program: string;
  partition: string;
  indicator: string;
  group_by: string[];
  period?: PeriodSpec;
  org_unit?: OrgUnitSelector;
  filters: FilterSpec[];
  limit?: number;
}

export type QueryRequest = QueryRequestAggregate | QueryRequestProgramIndicator;

export type AggregationSpec =
  | { op: 'count_star' }
  | { op: 'count'; column: string }
  | { op: 'count_distinct'; column: string }
  | { op: 'sum'; column: string }
  | { op: 'min'; column: string }
  | { op: 'max'; column: string }
  | { op: 'avg'; column: string };

export type PeriodSpec =
  | { range: { period_type: string; start: string; end: string } }
  | { exact: string }
  | { in: string[] };

export type OrgUnitSelector = { in: string[] } | { subtree: string[] };

export type FilterSpec =
  | { op: 'eq'; column: string; value: ValueJson }
  | { op: 'not_eq'; column: string; value: ValueJson }
  | { op: 'lt'; column: string; value: ValueJson }
  | { op: 'lt_eq'; column: string; value: ValueJson }
  | { op: 'gt'; column: string; value: ValueJson }
  | { op: 'gt_eq'; column: string; value: ValueJson }
  | { op: 'in'; column: string; values: ValueJson[] }
  | { op: 'is_null'; column: string }
  | { op: 'is_not_null'; column: string };

export interface OrderSpec {
  column: string;
  descending: boolean;
  nulls_first: boolean;
}

export interface ExplainPlan {
  root: OperatorNode;
  partition: string;
  dataset: string;
  metadata_revision: string;
}
export interface OperatorNode {
  name: string;
  details: string;
  estimated_rows?: number;
  children: OperatorNode[];
}

export interface EngineHealth {
  status: string;
  catalog_revision: string;
  partitions_open: number;
  query_history_size: number;
}

export interface CatalogSummary {
  revision: string;
}

export interface FreshnessResponse {
  dataset: string;
  partition: string;
  watermark_ms: number;
  data_revision: number;
  segment_count: number;
}

export interface DataProduct {
  id: string;
  owner: string;
  grain: string[];
  measures: string[];
  description: string;
  consumers: string[];
  freshness_contract: {
    max_staleness_seconds: number;
    consistency: ConsistencyMode;
  };
  sample_query?: QueryRequest;
}

export type JobStatus = 'PENDING' | 'RUNNING' | 'SUCCEEDED' | 'FAILED' | 'CANCELLED';

export interface JobRecord {
  id: string;
  kind: string;
  submitted_at_ms: number;
  started_at_ms?: number;
  finished_at_ms?: number;
  status: JobStatus;
  progress: unknown;
  result: unknown;
  error?: string;
  submitted_by?: string;
}
