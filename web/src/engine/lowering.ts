// PlanGraph -> DuckDB-Wasm SQL, by the SAME Rust code the server uses (crates/geo-lowering, RFC-004): it is compiled to WebAssembly and
// called from here. The browser no longer has its own compiler of the plan: the predicate trees, literal rules, BboxFilter (covering-box
// overlap), sort order (NULLS LAST), aggregates and the row cap are defined once, in Rust, for every engine.
//
// This module only moves JSON across the wasm boundary. The module is built by `npm run lowering` (web/scripts/build-lowering-wasm.mjs).
// No DOM and no bundler imports: the Vite loader (lowering-loader.ts) fetches the wasm, node tests read it from disk.

import { EngineError } from "./contract.ts";
import type { PlanGraph, SourceBinding } from "./contract.ts";

/** The operations the browser lowering accepts (the planner's capability list for this engine). Anything else is refused by the Rust code. */
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

/** One call across the boundary: request JSON in, response JSON out. */
function call(request: string): { ok: true; sql: string; files: string[] } | { ok: false; kind: string; message: string } {
  if (!api) throw new EngineError("engine", "the lowering module is not loaded (call initLowering first)");
  const bytes = enc.encode(request);
  const inPtr = api.gf_alloc(bytes.length);
  new Uint8Array(api.memory.buffer, inPtr, bytes.length).set(bytes);
  const packed = api.gf_lower_browser(inPtr, bytes.length);
  const outPtr = Number(packed >> 32n);
  const outLen = Number(packed & 0xffffffffn);
  const text = dec.decode(new Uint8Array(api.memory.buffer, outPtr, outLen));
  api.gf_free(outPtr, outLen);
  api.gf_free(inPtr, bytes.length);
  return JSON.parse(text);
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
  const r = call(request);
  if (!r.ok) throw new EngineError(r.kind === "unsupported" ? "unsupported" : "invalid", r.message);
  return {
    sql: r.sql,
    files: r.files.map((uri, i) => ({ name: `gf_src_${i}.parquet`, url: sources[uri].url, uri, access: sources[uri].access ?? "range" })),
  };
}
