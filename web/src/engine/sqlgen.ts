// PlanGraph (IR 0.2) -> DuckDB-Wasm SQL, for the subset the browser engine supports:
// Source, Filter, BboxFilter (through the bbox covering columns), Aggregate, Project, Limit, Sort.
// Everything else throws EngineError("unsupported") — the planner is expected to have routed
// such plans elsewhere; this is the executor's own safety net.
//
// Mirrors the semantics of crates/geo-engine-duckdb/src/sql.rs. A shared node is compiled once
// (one CTE, referenced by name). Every identifier is quoted; every literal is escaped or numeric.

import { EngineError } from "./contract.ts";
import type { Expr, Literal, NodeId, PlanGraph, PlanNode, SourceBinding } from "./contract.ts";

export const SUPPORTED_OPS = ["source", "filter", "bbox_filter", "aggregate", "project", "limit", "sort"];

export const qIdent = (s: string): string => `"${s.replace(/"/g, '""')}"`;
export const qStr = (s: string): string => `'${s.replace(/'/g, "''")}'`;

function num(v: number): string {
  if (!Number.isFinite(v)) throw new EngineError("invalid", `non-finite number ${v}`);
  return Number.isInteger(v) ? `${v}.0` : `${v}`;
}

function lit(l: Literal): string {
  if (l === "null") return "NULL";
  if ("int" in l) {
    if (!Number.isSafeInteger(l.int)) throw new EngineError("invalid", "integer literal out of the safe range");
    return String(l.int);
  }
  if ("float" in l) return num(l.float);
  if ("str" in l) return qStr(l.str);
  return l.bool ? "TRUE" : "FALSE";
}

const OPS = { eq: "=", ne: "<>", lt: "<", le: "<=", gt: ">", ge: ">=" } as const;

function expr(e: Expr): string {
  switch (e.kind) {
    case "compare":
      return `${qIdent(e.column)} ${OPS[e.op]} ${lit(e.value)}`;
    case "in":
      if (e.values.length === 0) throw new EngineError("invalid", "empty IN list");
      return `${qIdent(e.column)} IN (${e.values.map(lit).join(", ")})`;
    case "is_null":
      return `${qIdent(e.column)} IS NULL`;
    case "and":
    case "or":
      if (e.exprs.length === 0) throw new EngineError("invalid", "empty AND/OR");
      return e.exprs.map((x) => `(${expr(x)})`).join(e.kind === "and" ? " AND " : " OR ");
    case "not":
      return `NOT (${expr(e.expr)})`;
  }
}

interface Out {
  name: string;
  /** SQL column expressions of the covering (xmin, ymin, xmax, ymax) if still valid here. */
  covering: [string, string, string, string] | null;
  coveringGeom: string | null;
  coveringTop: string | null;
}

export interface Compiled {
  sql: string;
  /** Registered-file names used by the SQL, mapped from the plan's source uri. */
  files: { name: string; url: string; uri: string; access: "full" | "range" }[];
}

export function compile(plan: PlanGraph, sources: Record<string, SourceBinding>, maxRows: number): Compiled {
  const ctes: [string, string][] = [];
  const memo = new Map<NodeId, Out>();
  const files: Compiled["files"] = [];

  const push = (sql: string): string => {
    const n = `n${ctes.length}`;
    ctes.push([n, sql]);
    return n;
  };

  const visit = (id: NodeId, stack: Set<NodeId>): Out => {
    const done = memo.get(id);
    if (done) return done;
    if (stack.has(id)) throw new EngineError("invalid", `cycle at node ${id}`);
    const node: PlanNode | undefined = plan.nodes[id];
    if (!node) throw new EngineError("invalid", `missing node ${id}`);
    stack.add(id);
    const out = compileNode(node, stack);
    stack.delete(id);
    memo.set(id, out);
    return out;
  };

  const compileNode = (n: PlanNode, stack: Set<NodeId>): Out => {
    switch (n.op) {
      case "source": {
        const b = sources[n.uri];
        if (!b) throw new EngineError("invalid", "source is not bound to an authorized URL");
        let f = files.find((x) => x.uri === n.uri);
        if (!f) {
          f = { name: `gf_src_${files.length}.parquet`, url: b.url, uri: n.uri, access: b.access ?? "range" };
          files.push(f);
        }
        const c = b.covering;
        const col = (p: string) => p.split(".").map(qIdent).join(".");
        const geom = n.schema.geometry_columns[0]?.name ?? null;
        // Geometry leaves the engine as plain WKB bytes. DuckDB >= 1.5 types the column GEOMETRY(crs); streaming
        // such a column out of DuckDB-Wasm fails when the CRS is an identifier ("OGC:CRS84", e.g. Overture files):
        // "TransactionContext::ActiveTransaction called without active transaction" (reproduced, see ADR-009).
        // ST_AsWKB at the scan avoids the lazy typed conversion; it is part of the core build (no spatial extension).
        const name = push(geom ? `SELECT * REPLACE (ST_AsWKB(${qIdent(geom)}) AS ${qIdent(geom)}) FROM read_parquet(${qStr(f.name)})` : `SELECT * FROM read_parquet(${qStr(f.name)})`);
        return {
          name,
          covering: c ? [col(c.xmin), col(c.ymin), col(c.xmax), col(c.ymax)] : null,
          coveringGeom: geom,
          coveringTop: c ? c.xmin.split(".")[0] : null,
        };
      }
      case "filter": {
        const i = visit(n.input, stack);
        return { ...i, name: push(`SELECT * FROM ${qIdent(i.name)} WHERE ${expr(n.predicate)}`) };
      }
      case "bbox_filter": {
        const i = visit(n.input, stack);
        if (!i.covering || i.coveringGeom !== n.geometry_column) {
          throw new EngineError("unsupported", "bbox filter needs a bbox covering column (the browser has no spatial extension)");
        }
        const b = n.bbox;
        const [x0, y0, x1, y1] = [num(b.min_x), num(b.min_y), num(b.max_x), num(b.max_y)];
        const [cx0, cy0, cx1, cy1] = i.covering;
        const pred = `${cx0} <= ${x1} AND ${cx1} >= ${x0} AND ${cy0} <= ${y1} AND ${cy1} >= ${y0}`;
        return { ...i, name: push(`SELECT * FROM ${qIdent(i.name)} WHERE ${pred}`) };
      }
      case "aggregate": {
        const i = visit(n.input, stack);
        const sel = n.group_by.map(qIdent);
        for (const a of n.aggregates) {
          let f: string;
          if (a.func === "count") f = a.column === null ? "COUNT(*)" : `COUNT(${qIdent(a.column)})`;
          else if (a.column === null) throw new EngineError("invalid", "aggregate needs a column");
          else f = `${a.func.toUpperCase()}(${qIdent(a.column)})`;
          sel.push(`${f} AS ${qIdent(a.alias)}`);
        }
        let sql = `SELECT ${sel.join(", ")} FROM ${qIdent(i.name)}`;
        if (n.group_by.length) sql += ` GROUP BY ${n.group_by.map(qIdent).join(", ")}`;
        return { name: push(sql), covering: null, coveringGeom: null, coveringTop: null };
      }
      case "project": {
        const i = visit(n.input, stack);
        const name = push(`SELECT ${n.columns.map(qIdent).join(", ")} FROM ${qIdent(i.name)}`);
        const keep = i.coveringTop !== null && n.columns.includes(i.coveringTop) && i.coveringGeom !== null && n.columns.includes(i.coveringGeom);
        return keep ? { ...i, name } : { name, covering: null, coveringGeom: null, coveringTop: null };
      }
      case "limit": {
        const i = visit(n.input, stack);
        if (!Number.isSafeInteger(n.n) || n.n < 0) throw new EngineError("invalid", "bad limit");
        return { ...i, name: push(`SELECT * FROM ${qIdent(i.name)} LIMIT ${n.n}`) };
      }
      case "sort": {
        const i = visit(n.input, stack);
        if (n.keys.length === 0) throw new EngineError("invalid", "sort without keys");
        const ob = n.keys.map((k) => `${qIdent(k.column)}${k.descending ? " DESC" : ""}`).join(", ");
        return { ...i, name: push(`SELECT * FROM ${qIdent(i.name)} ORDER BY ${ob}`) };
      }
      default:
        throw new EngineError("unsupported", `operation '${n.op}' is not supported by the browser engine`);
    }
  };

  const root = visit(plan.root, new Set());
  const withClause = ctes.map(([n, s]) => `${qIdent(n)} AS NOT MATERIALIZED (${s})`).join(",\n");
  // fetch one row more than allowed so the caller can detect "result too large"
  const sql = `WITH ${withClause}\nSELECT * FROM ${qIdent(root.name)} LIMIT ${maxRows + 1}`;
  return { sql, files };
}
