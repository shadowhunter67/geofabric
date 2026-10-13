// Browser-side mirror of the Rust execution contract (RFC-002) and of the IR 0.2 wire format.
// The planner (Rust) decides *where* a plan runs; this file only describes what an executor
// in the browser must accept and return. Nothing here knows about the map UI.

export type NodeId = string;

export type Literal = { int: number } | { float: number } | { str: string } | { bool: boolean } | "null";
export type Expr =
  | { kind: "compare"; column: string; op: "eq" | "ne" | "lt" | "le" | "gt" | "ge"; value: Literal }
  | { kind: "in"; column: string; values: Literal[] }
  | { kind: "is_null"; column: string }
  | { kind: "and"; exprs: Expr[] }
  | { kind: "or"; exprs: Expr[] }
  | { kind: "not"; expr: Expr };

export type Crs = { epsg: number } | "ogc_crs84" | "undefined" | { unknown: string };
export interface BBox { min_x: number; min_y: number; max_x: number; max_y: number; crs: Crs }
export interface Field { name: string; data_type: unknown }
export interface Schema { fields: Field[]; geometry_columns: { name: string; geometry_type: string; crs: Crs }[] }
export interface AggExpr { func: "count" | "sum" | "min" | "max" | "avg"; column: string | null; alias: string }

export type PlanNode =
  | { op: "source"; uri: string; format: string; schema: Schema }
  | { op: "filter"; input: NodeId; predicate: Expr }
  | { op: "bbox_filter"; input: NodeId; geometry_column: string; bbox: BBox }
  | { op: "intersects"; input: NodeId; geometry_column: string; with: { wkt: string; crs: Crs } }
  | { op: "buffer"; input: NodeId }
  | { op: "spatial_join"; left: NodeId; right: NodeId }
  | { op: "aggregate"; input: NodeId; group_by: string[]; aggregates: AggExpr[] }
  | { op: "project"; input: NodeId; columns: string[] }
  | { op: "limit"; input: NodeId; n: number }
  | { op: "sort"; input: NodeId; keys: { column: string; descending?: boolean }[] };

export interface PlanGraph {
  ir_version: string;
  nodes: Record<NodeId, PlanNode>;
  root: NodeId;
  hints?: { preferred_engine?: string };
}

export interface Covering { xmin: string; ymin: string; xmax: string; ymax: string; declared: boolean }

/** What the gateway/planner tells the browser about each source. */
export interface SourceBinding {
  /** Authorized URL serving the Parquet bytes (HTTP range requests). */
  url: string;
  covering: Covering | null;
}

export type MetricSource = "engine_observed" | "source_observed" | "source_estimated" | "planner_estimated" | "derived" | "unknown";
export interface Metric<T> { value: T | null; source: MetricSource }

export interface ExecutionMetrics {
  elapsed_ms: Metric<number>;
  rows_returned: Metric<number>;
  /** Actual I/O is not measurable from inside the browser engine: always unknown. */
  bytes_read: Metric<number>;
  notes: string[];
}

export type EngineErrorKind = "unsupported" | "timeout" | "cancelled" | "limit" | "engine" | "invalid";

// (no TS parameter properties: the module must also load under node's type-stripping test runner)
export class EngineError extends Error {
  kind: EngineErrorKind;
  constructor(kind: EngineErrorKind, message: string) {
    super(message);
    this.kind = kind;
  }
}

export interface ExecutionContext {
  sources: Record<string, SourceBinding>; // keyed by the plan's source uri
  timeoutMs: number;
  maxRows: number;
  /** Planner's input estimate; lets the engine recycle its instance before memory accumulates. */
  estimatedInputBytes?: number;
  signal?: AbortSignal;
}

export interface ExecutionResult {
  /** Column names in order, with Arrow type strings. */
  columns: { name: string; type: string }[];
  /** Row-major cells exactly as returned by the engine (bigint for 64-bit ints, Uint8Array for binary). */
  rows: unknown[][];
  metrics: ExecutionMetrics;
  sql: string;
}

export interface Capabilities {
  id: string;
  kind: "browser";
  ops: string[];
}

export interface ExecutionEngine {
  id(): string;
  capabilities(): Capabilities;
  execute(plan: PlanGraph, ctx: ExecutionContext): Promise<ExecutionResult>;
}
