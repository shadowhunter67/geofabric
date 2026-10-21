// Static browser profile: the request -> PlanGraph step that the Rust gateway does in `build_ir` (crates/geo-gateway/src/lib.rs),
// for the query kinds a browser-only deployment can answer. Same IR (0.3), same node semantics, same limits as the gateway; what the
// browser engine cannot run (`intersects`, `buffer`, `transform`, spatial joins) is REFUSED with its reason, never approximated.
// No DOM, no fetch: unit-tested under node.

import type { Expr, Literal, PlanGraph, PlanNode, Schema } from "../engine/contract.ts";
import { SUPPORTED_OPS } from "../engine/sqlgen.ts";

/** Same cap as the gateway (`MAX_FEATURES`). */
export const MAX_FEATURES = 100_000;

export interface FilterReq { column: string; op: string; value?: unknown; values?: unknown[] }
export interface QueryReq {
  source: string; kind: string; bbox: [number, number, number, number]; crs: string;
  limit?: number; filter?: FilterReq; group_by?: string; sample_modulus?: number; engine?: string; browser?: boolean;
  with?: unknown; client?: unknown;
}

/** One column as `/api/sources` reports it. */
export interface ColumnMeta { name: string; type: unknown }
export interface RowGroupStat { index: number; rows: number; bytes: number; bbox: [number, number, number, number] | null }
/** A source of the static pack: its `SourceMeta` plus the public file and the per-row-group boxes read from its footer. */
export interface StaticSource {
  id: string; size_bytes: number; rows: number; row_groups: number; crs: string; geometry_column: string; geometry_type: string;
  bbox: [number, number, number, number] | null; columns: ColumnMeta[]; file: string; row_group_stats: RowGroupStat[];
}

/** An error the shim answers with the gateway's `{error, message}` shape and an HTTP-like status. */
export class StaticError extends Error {
  code: string;
  status: number;
  constructor(status: number, code: string, message: string) {
    super(message);
    this.code = code;
    this.status = status;
  }
}

/** What a browser-only deployment cannot do, said the same way everywhere (Explorer shows the localized sentence, Developer the detail). */
export const SERVER_ONLY: Record<string, string> = {
  intersects_features: "intersects (exact geometry test) is not supported by the browser engine",
  near: "metric distance needs Transform + SpatialSemiJoin, which the browser engine does not run",
  near_count: "metric distance needs Transform + SpatialSemiJoin, which the browser engine does not run",
  join_count_by: "a spatial join is not supported by the browser engine",
};

export function serverRequired(kind: string): StaticError {
  return new StaticError(422, "server_required", `This analysis requires GeoFabric Server (${kind}: ${SERVER_ONLY[kind] ?? "not supported by the browser engine"}). This deployment is browser-only: DuckDB server and PostGIS are not part of it.`);
}

function literal(v: unknown): Literal {
  if (v === null) return "null";
  if (typeof v === "boolean") return { bool: v };
  if (typeof v === "number") return Number.isInteger(v) ? { int: v } : { float: v };
  if (typeof v === "string") return { str: v };
  throw new StaticError(400, "bad_request", "filter value must be a string, number or boolean");
}

const COMPARE = new Set(["eq", "ne", "lt", "le", "gt", "ge"]);

export function filterExpr(f: FilterReq): Expr {
  if (f.op === "in") {
    if (!f.values || f.values.length === 0) throw new StaticError(400, "bad_request", "filter op 'in' needs a non-empty values list");
    return { kind: "in", column: f.column, values: f.values.map(literal) };
  }
  if (!COMPARE.has(f.op)) throw new StaticError(400, "bad_request", "unknown filter op");
  return { kind: "compare", column: f.column, op: f.op as "eq", value: literal(f.value) };
}

function dataType(t: unknown): unknown {
  return t;
}

export function sourceSchema(src: StaticSource): Schema {
  return {
    fields: src.columns.map((c) => ({ name: c.name, data_type: dataType(c.type) })),
    geometry_columns: [{ name: src.geometry_column, geometry_type: src.geometry_type, crs: "ogc_crs84" }],
  };
}

export interface BuiltPlan { ir: PlanGraph; kind: string; limit: number }

/**
 * The PlanGraph of one request. `uri` is the logical source key the physical plan binds to a URL.
 * Throws `StaticError` for a bad request or for a kind this deployment cannot answer.
 */
export function buildPlan(q: QueryReq, src: StaticSource, uri: string): BuiltPlan {
  if (q.kind in SERVER_ONLY) throw serverRequired(q.kind);
  if (!["features", "count", "count_by"].includes(q.kind)) throw new StaticError(400, "bad_request", "unknown kind");
  if (q.crs !== "OGC:CRS84" && q.crs !== "EPSG:4326") throw new StaticError(422, "crs_mismatch", `viewport CRS ${q.crs} differs from source CRS ${src.crs}; explicit transform not available in v0`);
  const [x0, y0, x1, y1] = q.bbox;
  if (![x0, y0, x1, y1].every(Number.isFinite) || x0 > x1 || y0 > y1) throw new StaticError(400, "bad_request", "bad bbox");

  const nodes: Record<string, PlanNode> = {};
  let n = 0;
  const add = (node: PlanNode): string => { const id = `n${++n}`; nodes[id] = node; return id; };
  const schema = sourceSchema(src);
  let cur = add({
    op: "source", uri, format: "geo_parquet", schema,
    ...(src.bbox ? { statistics: { bbox: { crs: "ogc_crs84", min_x: src.bbox[0], min_y: src.bbox[1], max_x: src.bbox[2], max_y: src.bbox[3] }, row_count: src.rows, row_groups: src.row_groups, total_bytes: src.size_bytes } } : {}),
  } as PlanNode);
  cur = add({ op: "bbox_filter", input: cur, geometry_column: src.geometry_column, bbox: { crs: "ogc_crs84", min_x: x0, min_y: y0, max_x: x1, max_y: y1 } });
  if (q.filter) cur = add({ op: "filter", input: cur, predicate: filterExpr(q.filter) });
  if (q.sample_modulus !== undefined) {
    const m = q.sample_modulus;
    if (q.kind !== "features") throw new StaticError(400, "bad_request", "sample_modulus is only valid for row-returning kinds: counts and aggregates always use the full result");
    if (!Number.isInteger(m) || m < 1 || (m & (m - 1)) !== 0 || m > 1 << 20) throw new StaticError(400, "bad_request", "sample_modulus must be a power of two between 1 and 1048576");
    if (m > 1) {
      if (!src.columns.some((c) => c.name === "id")) throw new StaticError(400, "bad_request", "sampling needs an `id` column in the source");
      cur = add({ op: "filter", input: cur, predicate: { kind: "hash_bucket", column: "id", modulus: m, remainder: 0 } });
    }
  }
  const limit = Math.min(q.limit ?? 20_000, MAX_FEATURES);
  let root: string;
  if (q.kind === "count") {
    root = add({ op: "aggregate", input: cur, group_by: [], aggregates: [{ func: "count", column: null, alias: "count" }] });
  } else if (q.kind === "count_by") {
    const g = q.group_by;
    if (!g) throw new StaticError(400, "bad_request", "group_by required");
    if (!src.columns.some((c) => c.name === g)) throw new StaticError(422, "schema", `unknown column ${g}`);
    const agg = add({ op: "aggregate", input: cur, group_by: [g], aggregates: [{ func: "count", column: null, alias: "count" }] });
    // the group key breaks ties, so the top 50 is cut at the same place on every engine
    const sorted = add({ op: "sort", input: agg, keys: [{ column: "count", descending: true }, { column: g, descending: false }] });
    root = add({ op: "limit", input: sorted, n: 50 });
  } else {
    const columns = src.columns.filter((c) => !(typeof c.type === "object" && c.type !== null && (c.type as { other?: string }).other !== "geometry")).map((c) => c.name);
    const proj = add({ op: "project", input: cur, columns });
    root = add({ op: "limit", input: proj, n: limit });
  }
  const ir: PlanGraph = { ir_version: "0.3", nodes, root, ...(q.engine ? { hints: { preferred_engine: q.engine } } : {}) };
  const unsupported = Object.values(nodes).filter((x) => !SUPPORTED_OPS.includes(x.op));
  if (unsupported.length) throw serverRequired(unsupported[0].op);
  return { ir, kind: q.kind, limit };
}

/** Row groups of `src` that can hold rows inside `bbox` (a row group without a box is always kept: nothing proves it can be skipped). */
export function prune(src: StaticSource, bbox: [number, number, number, number]) {
  const [x0, y0, x1, y1] = bbox;
  const kept = src.row_group_stats.filter((g) => !g.bbox || (g.bbox[0] <= x1 && g.bbox[2] >= x0 && g.bbox[1] <= y1 && g.bbox[3] >= y0));
  const sum = (f: (g: RowGroupStat) => number, gs: RowGroupStat[]) => gs.reduce((a, g) => a + f(g), 0);
  return {
    kept: kept.map((g) => g.index), skipped: src.row_group_stats.length - kept.length, total_row_groups: src.row_group_stats.length,
    total_rows: sum((g) => g.rows, src.row_group_stats), kept_rows: sum((g) => g.rows, kept),
    kept_bytes: sum((g) => g.bytes, kept), total_bytes: src.size_bytes, method: "covering_stats",
  };
}
