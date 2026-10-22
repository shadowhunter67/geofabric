// Request -> PlanGraph (crates/geo-plan) and PlanGraph -> DuckDB-Wasm SQL (crates/geo-lowering), by the SAME Rust code the server uses (RFC-004):
// one WebAssembly module, called from here. The browser no longer has its own compiler of the plan: the predicate trees, literal rules, BboxFilter (covering-box
// overlap), sort order (NULLS LAST), aggregates and the row cap are defined once, in Rust, for every engine.
//
// This module only moves JSON across the wasm boundary. The module is built by `npm run lowering` (web/scripts/build-lowering-wasm.mjs).
// No DOM and no bundler imports: the Vite loader (lowering-loader.ts) fetches the wasm, node tests read it from disk.

import { EngineError } from "./contract.ts";
import type { PlanGraph, SourceBinding } from "./contract.ts";

/**
 * The operations the browser lowering accepts. The Rust constant `geo_lowering::browser::SUPPORTED_OPS` is the source of truth; this copy exists
 * only because the engine reports its capabilities synchronously, and `web/test/plan.test.ts` fails if it ever differs from the wasm's list.
 */
export const SUPPORTED_OPS = ["source", "filter", "bbox_filter", "aggregate", "project", "limit", "sort"];

export interface Compiled {
  sql: string;
  /** Registered-file names used by the SQL, mapped from the plan's source uri. */
  files: { name: string; url: string; uri: string; access: "full" | "range" }[];
}

interface Exports {
  memory: WebAssembly.Memory;
  gf_alloc(len: number): number;
  gf_free(ptr: number, len: number): void;
  gf_lower_browser(ptr: number, len: number): bigint;
  gf_build_plan(ptr: number, len: number): bigint;
  gf_browser_ops(): bigint;
}

let api: Exports | null = null;

/** Instantiate the lowering module (once). `source` is the wasm bytes, or a (promise of a) fetch Response. */
export async function initLowering(source: BufferSource | Response | Promise<Response>): Promise<void> {
  if (api) return;
  const resolved = await source;
  let instance: WebAssembly.Instance;
  if (resolved instanceof Response) {
    const copy = resolved.clone(); // streaming consumes the body; a wrong Content-Type (some static hosts) falls back to the bytes
    instance = (await WebAssembly.instantiateStreaming(resolved, {}).catch(async () => WebAssembly.instantiate(await copy.arrayBuffer(), {}))).instance;
  } else {
    instance = (await WebAssembly.instantiate(resolved, {})).instance;
  }
  api = instance.exports as unknown as Exports;
}

export function loweringReady(): boolean {
  return api !== null;
}

const enc = new TextEncoder();
const dec = new TextDecoder();

/** One call across the boundary: request JSON in, response JSON out (`fn` selects the exported function). */
function call<T>(fn: "gf_lower_browser" | "gf_build_plan", request: string): T {
  if (!api) throw new EngineError("engine", "the lowering module is not loaded (call initLowering first)");
  const bytes = enc.encode(request);
  const inPtr = api.gf_alloc(bytes.length);
  new Uint8Array(api.memory.buffer, inPtr, bytes.length).set(bytes);
  const packed = api[fn](inPtr, bytes.length);
  const outPtr = Number(packed >> 32n);
  const outLen = Number(packed & 0xffffffffn);
  const text = dec.decode(new Uint8Array(api.memory.buffer, outPtr, outLen));
  api.gf_free(outPtr, outLen);
  api.gf_free(inPtr, bytes.length);
  return JSON.parse(text) as T;
}

/** The browser operation list as the Rust code reports it (used by tests to pin `SUPPORTED_OPS`). */
export function browserOpsFromWasm(): string[] {
  if (!api) throw new EngineError("engine", "the lowering module is not loaded (call initLowering first)");
  const packed = api.gf_browser_ops();
  const ptr = Number(packed >> 32n);
  const len = Number(packed & 0xffffffffn);
  const text = dec.decode(new Uint8Array(api.memory.buffer, ptr, len));
  api.gf_free(ptr, len);
  return JSON.parse(text) as string[];
}

/** A request the canonical builder refused. `code` is the structured contract (UNKNOWN_DATASET, INVALID_FIELD, ...); `status` the HTTP status a gateway would answer with. */
export class PlanBuildError extends Error {
  code: string;
  status: number;
  legacy: string;
  constructor(code: string, message: string, status: number, legacy: string) {
    super(message);
    this.name = "PlanBuildError";
    this.code = code;
    this.status = status;
    this.legacy = legacy;
  }
}

export interface BuiltPlan {
  /** the PlanGraph exactly as the native builder serializes it (compare as text) */
  planJson: string;
  plan: PlanGraph;
  /** operations of the plan the browser cannot run (empty = fully supported): the capability verdict, never an approximation */
  browserMissingOps: string[];
}

/**
 * Request -> PlanGraph by the canonical builder (Rust, `geo-plan`). `sources` maps each dataset id the request names to its Source node.
 * Throws PlanBuildError for a refused request; never builds an invalid plan.
 */
export function buildPlan(request: unknown, sources: Record<string, unknown>): BuiltPlan {
  const r = call<{ ok: true; plan_json: string; browser_missing_ops: string[] } | { ok: false; code?: string; kind?: string; message: string; status?: number; error?: string }>(
    "gf_build_plan",
    JSON.stringify({ request, sources }),
  );
  if (!r.ok) throw new PlanBuildError(r.code ?? "INVALID_REQUEST", r.message, r.status ?? 400, r.error ?? "bad_request");
  return { planJson: r.plan_json, plan: JSON.parse(r.plan_json) as PlanGraph, browserMissingOps: r.browser_missing_ops };
}

/**
 * Lower `plan` for DuckDB-Wasm. Throws EngineError("unsupported") for an operation this dialect cannot run (the planner is expected to
 * have routed it elsewhere: this is the executor's safety net) and EngineError("invalid") for a malformed plan.
 */
export function compile(plan: PlanGraph, sources: Record<string, SourceBinding>, maxRows: number): Compiled {
  const request = JSON.stringify({
    plan,
    sources: Object.fromEntries(Object.entries(sources).map(([uri, b]) => [uri, { covering: b.covering ? [b.covering.xmin, b.covering.ymin, b.covering.xmax, b.covering.ymax] : null }])),
    max_rows: maxRows,
  });
  const r = call<{ ok: true; sql: string; files: string[] } | { ok: false; kind: string; message: string }>("gf_lower_browser", request);
  if (!r.ok) throw new EngineError(r.kind === "unsupported" ? "unsupported" : "invalid", r.message);
  return {
    sql: r.sql,
    files: r.files.map((uri, i) => ({ name: `gf_src_${i}.parquet`, url: sources[uri].url, uri, access: sources[uri].access ?? "range" })),
  };
}
