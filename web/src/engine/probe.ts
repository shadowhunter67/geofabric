// Runtime capability probe for the browser engine (ADR-009).
//
// Static capabilities say what DuckDB-Wasm is *designed* for. The probe shows what THIS runtime in THIS tab
// demonstrably does right now: it instantiates the engine, then runs a battery of tiny checks and reports the
// outcome to the gateway. The checks run the engine's real execution path (the PlanGraph -> SQL compiler and the
// streaming result reader) on two fixture files, one per GeoParquet writer class, so a defect anywhere on that path
// (for instance streaming a typed GEOMETRY column) shows up here instead of in a user's query.

import * as duckdb from "@duckdb/duckdb-wasm";
import { wkbToGeoJSON } from "./wkb.ts";
import { SUPPORTED_OPS } from "./sqlgen.ts";
import type { DuckDbWasmEngine } from "./wasm.ts";
import { ENGINE_ID } from "./wasm.ts";
import { deviceProfileFrom, memoryClassFor } from "./probe-model.ts";
import type { ProbeCheck, ProbeReport } from "./probe-model.ts";
import type { ExecutionContext, PlanGraph, PlanNode } from "./contract.ts";

export { isFresh, usable, sessionId, PROBE_TTL_MS, memoryClassFor } from "./probe-model.ts";
export type { ProbeCheck, ProbeReport, MemoryClass, DeviceProfile } from "./probe-model.ts";

const FIXTURES = { plain_wkb: "probe/probe-plain-wkb.parquet", duckdb_typed: "probe/probe-duckdb-typed.parquet" } as const;
const STEP_TIMEOUT_MS = 20_000;
const COVERING = { xmin: "bbox.xmin", ymin: "bbox.ymin", xmax: "bbox.xmax", ymax: "bbox.ymax", declared: true };

async function step(name: string, required: boolean, fn: () => Promise<string>): Promise<ProbeCheck> {
  const t0 = performance.now();
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    const detail = await Promise.race([
      fn(),
      new Promise<string>((_, rej) => { timer = setTimeout(() => rej(new Error(`timed out after ${STEP_TIMEOUT_MS} ms`)), STEP_TIMEOUT_MS); }),
    ]);
    return { name, ok: true, required, detail, ms: performance.now() - t0 };
  } catch (e) {
    return { name, ok: false, required, detail: (e instanceof Error ? e.message : String(e)).slice(0, 300), ms: performance.now() - t0 };
  } finally {
    clearTimeout(timer);
  }
}

/** WebAssembly SIMD support (feature-detection by validating a tiny module that uses a v128 instruction). */
function wasmSimd(): boolean {
  try {
    return WebAssembly.validate(new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0, 1, 5, 1, 96, 0, 1, 123, 3, 2, 1, 0, 10, 10, 1, 8, 0, 65, 0, 253, 15, 253, 98, 11]));
  } catch {
    return false;
  }
}

type PerfWithMemory = Performance & { memory?: { jsHeapSizeLimit: number } };

/** A chain of operators over one fixture, as a PlanGraph (what the planner would send). */
function chain(uri: string, ...ops: ((input: string) => PlanNode)[]): PlanGraph {
  const nodes: Record<string, PlanNode> = {
    p0: {
      op: "source", uri, format: "geo_parquet",
      schema: { fields: [{ name: "geometry", data_type: { other: "geometry" } }], geometry_columns: [{ name: "geometry", geometry_type: "polygon", crs: "ogc_crs84" }] },
    },
  };
  let last = "p0";
  ops.forEach((mk, i) => { const id = `p${i + 1}`; nodes[id] = mk(last); last = id; });
  return { ir_version: "0.2", nodes, root: last };
}
const limit = (n: number) => (input: string): PlanNode => ({ op: "limit", input, n });
const bbox = (input: string): PlanNode => ({ op: "bbox_filter", input, geometry_column: "geometry", bbox: { min_x: 104, min_y: 10, max_x: 106, max_y: 12, crs: "ogc_crs84" } });
const count = (input: string): PlanNode => ({ op: "aggregate", input, group_by: [], aggregates: [{ func: "count", column: null, alias: "n" }] });

/**
 * Run the probe on `engine`. Never throws: a runtime that cannot even start is reported as such
 * (failed required checks), because "the browser engine is unusable" is exactly what the planner must learn.
 */
export async function runProbe(engine: DuckDbWasmEngine, session: string): Promise<ProbeReport> {
  const t0 = performance.now();
  const checks: ProbeCheck[] = [];
  const run = async (name: string, required: boolean, fn: () => Promise<string>) => {
    const c = await step(name, required, fn);
    checks.push(c);
    return c;
  };
  const dm = (navigator as Navigator & { deviceMemory?: number }).deviceMemory ?? null;
  const heap = (performance as PerfWithMemory).memory?.jsHeapSizeLimit;
  let runtimeVersion = "unknown";
  const uri = (cls: keyof typeof FIXTURES | "plain_wkb_range") => `probe:${cls}`;
  const ctx = (): ExecutionContext => ({
    sources: {
      [uri("plain_wkb")]: { url: FIXTURES.plain_wkb, covering: COVERING, access: "full" },
      [uri("duckdb_typed")]: { url: FIXTURES.duckdb_typed, covering: COVERING, access: "full" },
      [uri("plain_wkb_range")]: { url: FIXTURES.plain_wkb, covering: COVERING, access: "range" },
    },
    timeoutMs: STEP_TIMEOUT_MS - 2_000,
    maxRows: 1000,
  });
  const exec = (plan: PlanGraph) => engine.execute(plan, ctx());
  const geometryOf = async (cls: keyof typeof FIXTURES) => {
    const r = await exec(chain(uri(cls), limit(1)));
    const gi = r.columns.findIndex((c) => c.name === "geometry");
    const g = gi >= 0 ? r.rows[0]?.[gi] : undefined;
    if (!(g instanceof Uint8Array)) throw new Error(`geometry column is ${g == null ? "missing/null" : typeof g}, not WKB bytes`);
    const gj = wkbToGeoJSON(g);
    if (!gj) throw new Error("WKB bytes did not decode");
    return gj.type;
  };
  const scalar = async (plan: PlanGraph, col = "n") => {
    const r = await exec(plan);
    const i = r.columns.findIndex((c) => c.name === col);
    return Number(r.rows[0]?.[i]);
  };

  await run("worker", true, async () => {
    if (typeof Worker === "undefined") throw new Error("Web Workers are not available");
    return "Worker available";
  });
  let db: duckdb.AsyncDuckDB | null = null;
  const init = await run("init", true, async () => {
    db = await engine.instance();
    const conn = await db.connect();
    try {
      const t = await conn.query("SELECT version() AS v");
      runtimeVersion = String(t.get(0)?.v ?? "unknown");
    } finally {
      await conn.close();
    }
    return `DuckDB ${runtimeVersion}`;
  });

  if (init.ok && db) {
    const d = db as duckdb.AsyncDuckDB;
    await run("arrow", true, async () => {
      const conn = await d.connect();
      try {
        const t = await conn.query("SELECT 1 AS a, 'x' AS b");
        if (t.numRows !== 1 || t.schema.fields.length !== 2) throw new Error("unexpected Arrow table shape");
        return `Arrow table ${t.numRows}x${t.schema.fields.length}`;
      } finally {
        await conn.close();
      }
    });
    await run("extension_load", false, async () => {
      // information only: which extensions this build ships (parquet is built in and loads on first use)
      const conn = await d.connect();
      try {
        const t = await conn.query("SELECT extension_name AS e, installed AS i FROM duckdb_extensions() WHERE extension_name IN ('parquet', 'spatial')");
        const rows: string[] = [];
        for (let i = 0; i < t.numRows; i++) rows.push(`${t.get(i)?.e}=${t.get(i)?.i ? "built in" : "absent"}`);
        return rows.join(", ") || "no extension list";
      } finally {
        await conn.close();
      }
    });
    await run("parquet", true, async () => {
      const rows = await scalar(chain(uri("plain_wkb"), count));
      if (rows !== 64) throw new Error(`expected 64 rows, read ${rows}`);
      return "compiled plan read 64 rows over HTTP range requests";
    });
    await run("http_range", false, async () => {
      // the large-file path: HTTP range requests (HEAD + ranged GETs) must reach the gateway and return the right bytes
      const rows = await scalar(chain(uri("plain_wkb_range"), bbox, count));
      if (rows !== 64) throw new Error(`range-mode read returned ${rows} of 64 rows`);
      return "range-request reads work";
    });
    await run("geometry_wkb", true, async () => `decoded ${await geometryOf("plain_wkb")} from streamed WKB bytes`);
    await run("covering", true, async () => {
      const rows = await scalar(chain(uri("plain_wkb"), bbox, count));
      if (rows !== 64) throw new Error(`covering filter returned ${rows} of 64 rows`);
      return "bbox covering columns filterable";
    });
    // reader-compatibility classes (geo_source_geoparquet::ReadClass): information, not required
    await run("read_class:plain_wkb", false, async () => `reads plain WKB files (${await geometryOf("plain_wkb")})`);
    await run("read_class:duckdb_typed", false, async () => `reads DuckDB>=1.5 typed-geometry files (${await geometryOf("duckdb_typed")})`);
    // operations: the same compiler/executor path, one tiny plan per operation kind
    const plain = uri("plain_wkb");
    const ops: Record<string, PlanGraph> = {
      source: chain(plain, limit(1)),
      filter: chain(plain, (input) => ({ op: "filter", input, predicate: { kind: "compare", column: "id", op: "lt", value: { int: 10 } } }), count),
      bbox_filter: chain(plain, bbox, count),
      aggregate: chain(plain, (input) => ({ op: "aggregate", input, group_by: ["category"], aggregates: [{ func: "count", column: null, alias: "n" }] })),
      project: chain(plain, (input) => ({ op: "project", input, columns: ["id", "category"] }), limit(1)),
      limit: chain(plain, limit(3)),
      sort: chain(plain, (input) => ({ op: "sort", input, keys: [{ column: "id", descending: true }] }), limit(1)),
    };
    for (const op of SUPPORTED_OPS) {
      const plan = ops[op];
      if (plan) await run(`op:${op}`, false, async () => { await exec(plan); return "ran"; });
    }
  }

  const supported = SUPPORTED_OPS.filter((op) => checks.find((c) => c.name === `op:${op}`)?.ok);
  return {
    engine: ENGINE_ID,
    session,
    engine_version: duckdb.PACKAGE_VERSION,
    runtime_version: runtimeVersion,
    captured_at_ms: Date.now(),
    duration_ms: performance.now() - t0,
    checks,
    memory_class: memoryClassFor(dm),
    device_memory_gb: dm,
    js_heap_limit_mb: heap ? Math.round(heap / 1048576) : null,
    supported_ops: supported,
    device: deviceProfileFrom(navigator as never, wasmSimd(), typeof SharedArrayBuffer !== "undefined" && (globalThis as { crossOriginIsolated?: boolean }).crossOriginIsolated === true),
  };
}
