// Static browser deployment profile (ADR-014): the `/api/*` contract the UI already speaks, answered in the page from files published
// next to it. There is no server: the only engine is DuckDB-Wasm, so the "planner" has exactly one feasible candidate and says so;
// DuckDB-server and PostGIS are reported as NOT AVAILABLE IN THIS DEPLOYMENT (a capability that is absent, not an error), and
// anything that needs them is refused with its reason, never approximated and never faked.
//
// The UI, the PlanGraph (IR 0.3), the browser lowering and the executor are the same code as in the full stack: this module only
// replaces the HTTP boundary. `handle` is pure over its inputs (testable under node); `installStaticGateway` wires it to `fetch`.

import type { Assessment, Candidate, Health, Physical, RunResp, Scan } from "../types.ts";
import { isFresh, usable } from "../engine/probe-model.ts";
import type { ProbeReport } from "../engine/probe-model.ts";
import { StaticError, planRequest, prune, serverRequired } from "./adapter.ts";
import type { QueryReq, StaticSource } from "./adapter.ts";

export const NOT_IN_DEPLOYMENT = "not available in this deployment (static browser profile: no GeoFabric Server)";
const COVERING = { xmin: "bbox.xmin", ymin: "bbox.ymin", xmax: "bbox.xmax", ymax: "bbox.ymax", declared: false };
/**
 * Whole-file ("full") vs HTTP range access is decided by what the DATA HOST was verified to support (the manifest's `host.range`, written by
 * data/scripts/publish_demo_data.py after real HEAD/GET Range checks). On a host that answers ranged requests correctly, layers bigger than
 * RANGE_MIN are read by range (only the row groups a query needs cross the network); tiny files are cheaper whole. On a host that does not
 * (GitHub Pages answers the ranged HEAD with 200, ADR-014) every layer is fetched whole, once.
 */
const RANGE_MIN = 1024 * 1024;
const WHOLE_FILE_MAX = 32 * 1024 * 1024;

/** The public data manifest (data/public-demo/pack.json, schema 2): where each layer lives, what it is, and what the host was verified to do. */
export interface DataManifest {
  version: string;
  host: { kind: string; base_url: string; range: boolean; commit?: string } | null;
  files: Record<string, { file: string; path: string; url?: string; bytes: number; sha256: string }>;
}
export interface StaticPack { sources: StaticSource[]; catalog: unknown; manifest: DataManifest }
export interface GatewayDeps {
  pack: StaticPack;
  /** absolute URL of a layer's file (from the manifest; a bundled pack resolves against the page, so a non-root base path works) */
  fileUrl(sourceId: string): string;
}

export interface GatewayState {
  probe: ProbeReport | null;
  events: number;
  failures: number;
  lastFailure: string | null;
}

export function newState(): GatewayState {
  return { probe: null, events: 0, failures: 0, lastFailure: null };
}

function health(st: GatewayState): Record<string, Health> {
  const wasm: Health = st.events === 0
    ? { state: "unknown", reason: "no recent observation", events: 0, failures: 0, last_failure: null }
    : {
        state: st.failures === 0 ? "healthy" : st.failures >= 3 ? "unhealthy" : "degraded",
        reason: `${st.events - st.failures} of the last ${st.events} runs succeeded`, events: st.events, failures: st.failures, last_failure: st.lastFailure,
      };
  const absent: Health = { state: "unknown", reason: NOT_IN_DEPLOYMENT, events: 0, failures: 0, last_failure: null };
  return { "duckdb-wasm": wasm, duckdb: absent, postgis: absent };
}

function probeInfo(st: GatewayState) {
  const p = st.probe;
  const status = !p ? "missing" : isFresh(p) ? "fresh" : "stale";
  return {
    status, usable: !!p && usable(p), age_ms: p ? Date.now() - p.captured_at_ms : null,
    failed_checks: p ? p.checks.filter((c) => !c.ok && c.required).map((c) => c.name) : [],
    memory_class: p?.memory_class ?? null, engine_version: p?.engine_version ?? null, runtime_version: p?.runtime_version ?? null,
  };
}

function absentCandidate(engine: string): Candidate {
  return { engine, kind: "server", feasible: false, score_ms: null, cost: null, reasons: [], rejected_because: [NOT_IN_DEPLOYMENT], confidence: null, risk: null, health: health(newState())[engine], probe_status: "not_required" };
}

function buildPhysical(q: QueryReq, src: StaticSource, uri: string, st: GatewayState, nodeCount: number, rangeHost: boolean): Physical {
  const pr = prune(src, q.bbox);
  const scan: Scan = {
    uri, total_bytes: src.size_bytes, has_histogram: false, covering: COVERING,
    prune: pr,
  };
  const strategy: "full" | "range" = rangeHost ? (src.size_bytes > RANGE_MIN ? "range" : "full") : src.size_bytes <= WHOLE_FILE_MAX ? "full" : "range";
  const footer = 64 * 1024;
  const fetchEst = strategy === "full" ? src.size_bytes : pr.kept_bytes + footer;
  const probe = probeInfo(st);
  const browserOk = probe.status !== "fresh" || probe.usable;
  const wasm: Candidate = {
    engine: "duckdb-wasm", kind: "browser", feasible: browserOk, score_ms: null, cost: null,
    reasons: [
      "supports all required operation kinds",
      "runs next to the user: DuckDB-Wasm in this page, reading the published GeoParquet",
      "the only engine in this deployment, so no cost comparison is made (no estimate is invented)",
    ],
    rejected_because: browserOk ? [] : ["runtime probe failed"], confidence: null, risk: null,
    health: health(st)["duckdb-wasm"], probe_status: probe.status,
  };
  const candidates: Candidate[] = [wasm, absentCandidate("duckdb"), absentCandidate("postgis")];
  const rows = pr.kept_rows;
  const assessment: Assessment = {
    confidence: {
      score: 0.7, label: "medium",
      factors: [
        { name: "single_engine", value: 1, weight: 0.3, detail: "one feasible engine: nothing to choose between" },
        { name: "estimate_quality", value: 0.4, weight: 0.4, detail: "row-group statistics only; no histogram in the static pack" },
        { name: "probe_freshness", value: probe.status === "fresh" ? 1 : 0.5, weight: 0.3, detail: `runtime probe ${probe.status}` },
      ],
    },
    engine_confidence: 0.7, margin_factor: 1,
    estimate: {
      rows, low: 0, high: rows, quality: "poor", cardinality: rows, cardinality_low: 0, cardinality_high: rows, cardinality_quality: "poor",
      bounded_by_limit: false, limit_certain: false, covering_quality: "Inferred", layout: "unknown", raw_selected: rows,
      correction: { factor: 1, samples: 0, basis: "none", spread: 0 }, spatial_coherence: null,
      row_groups_total: pr.total_row_groups, row_groups_kept: pr.kept.length,
      notes: ["upper bound from the row groups the viewport can touch"],
    },
    network: { name: "static", rtt_ms: 0, down_mbps: 0, up_mbps: 0, origin: "unmeasured" },
    health: health(st),
    probes: { "duckdb-wasm": probe },
    fallback: null,
    policy: [],
    browser_access: [{ uri, strategy, file_bytes: src.size_bytes, requests_est: strategy === "full" ? 1 : 2 + pr.kept.length, fetch_bytes_est: fetchEst, fetch_bytes_upper: strategy === "full" ? src.size_bytes : Math.min(src.size_bytes, pr.kept_bytes + footer), row_bytes: src.rows ? src.size_bytes / src.rows : 0 }],
  };
  return {
    engine: "duckdb-wasm", placement: "browser",
    decisions: [
      { rule: "validate", subject: "plan", outcome: "info", reason: `IR valid (${nodeCount} nodes)` },
      { rule: "deployment_profile", subject: "engines", outcome: "info", reason: `static browser profile: DuckDB server and PostGIS are ${NOT_IN_DEPLOYMENT}` },
      { rule: "spatial_prune", subject: uri, outcome: "info", reason: `${pr.kept.length} of ${pr.total_row_groups} row groups can hold rows in the viewport (row-group statistics); ${pr.kept_bytes} of ${src.size_bytes} bytes remain` },
    ],
    steps: [
      { label: "Planner selected the browser engine", detail: "the only engine in a static deployment" },
      strategy === "full"
        ? { label: "Fetch the GeoParquet file", detail: "the published file is downloaded once, on first use (this host cannot serve range reads); DuckDB-Wasm then reads only the row groups it needs from memory" }
        : { label: "Fetch GeoParquet byte ranges", detail: "HTTP range requests to the published file; DuckDB-Wasm reads the row groups it needs" },
      { label: "Compile logical plan -> DuckDB-Wasm SQL (client side)", detail: "the client translates the same PlanGraph; bbox filters use the bbox covering column" },
    ],
    assessment, candidates, input_bytes_upper_bound: fetchEst, result_rows_estimate: rows, scans: [scan],
  };
}

async function sha256Hex(text: string): Promise<string> {
  const buf = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

const empty = { elapsed_ms: { value: null, source: "unknown" }, rows_returned: { value: null, source: "unknown" }, row_groups_total: { value: null, source: "unknown" }, row_groups_skipped: { value: null, source: "unknown" }, rows_scanned_upper_bound: { value: null, source: "unknown" }, estimated_bytes_upper_bound: { value: null, source: "unknown" }, bytes_read: { value: null, source: "unknown" }, peak_memory_bytes: { value: null, source: "unknown" }, notes: [] as string[] };

async function query(q: QueryReq, deps: GatewayDeps, st: GatewayState): Promise<RunResp> {
  if (q.engine === "duckdb" || q.engine === "postgis") throw new StaticError(422, "server_required", `This analysis requires GeoFabric Server: the ${q.engine} engine is ${NOT_IN_DEPLOYMENT}.`);
  if (q.browser === false) throw new StaticError(422, "server_required", `This analysis requires GeoFabric Server: browser execution was declined and there is no other engine (${NOT_IN_DEPLOYMENT}).`);
  // request -> PlanGraph by the canonical builder (Rust, as WebAssembly); then the capability verdict: a plan the browser cannot run is refused, not approximated
  const planned = planRequest(q, deps.pack.sources);
  if (planned.missingOps.length) throw serverRequired(planned.missingOps);
  const src = deps.pack.sources.find((s) => s.id === q.source)!;
  const uri = planned.uri;
  const planId = await sha256Hex(JSON.stringify(planned.plan));
  const physical = buildPhysical(q, src, uri, st, Object.keys(planned.plan.nodes).length, deps.pack.manifest.host?.range === true);
  return {
    placement: "browser", plan_id: planId, ir: planned.plan, physical,
    lineage: Object.entries(planned.plan.nodes).map(([id, n]) => ({ id, op: n.op })),
    source_urls: { [uri]: deps.fileUrl(src.id) },
    limits: { max_rows: 100_000, timeout_ms: 30_000 },
    columns: [], rows: [], rows_total: 0, geojson: null, metrics: empty as RunResp["metrics"],
    timing: { plan_and_overhead_ms: { value: 0, source: "derived" }, total_ms: { value: 0, source: "derived" }, boundary_conversion_ms: { value: 0, source: "derived" } },
  };
}

export interface Reply { status: number; body: unknown }

/** One `/api/*` call. `path` starts with `/api/`; `body` is the parsed JSON body of a POST (or undefined). */
export async function handle(method: string, path: string, body: unknown, deps: GatewayDeps, st: GatewayState): Promise<Reply> {
  const ok = (b: unknown): Reply => ({ status: 200, body: b });
  try {
    if (method === "GET") {
      if (path === "/api/health") return ok({ ok: true, profile: "static-browser" });
      if (path === "/api/catalog") return ok(deps.pack.catalog);
      if (path === "/api/sources") return ok({ sources: deps.pack.sources.map(({ file: _f, row_group_stats: _r, ir_source: _i, ...meta }) => ({ format: "GeoParquet", crs_known: true, has_covering: true, covering_inferred: true, modified: null, ...meta })) });
      const rg = path.match(/^\/api\/sources\/([^/]+)\/row_groups$/);
      if (rg) {
        const s = deps.pack.sources.find((x) => x.id === decodeURIComponent(rg[1]));
        if (!s) throw new StaticError(404, "no_such_source", rg[1]);
        return ok({ crs: s.crs, row_groups: s.row_group_stats });
      }
      if (path === "/api/state") {
        return ok({
          profile: "static-browser", engines: ["duckdb-wasm"], health: health(st), probes: { "duckdb-wasm": probeInfo(st) },
          postgis: { configured: false }, server: { available: false },
        });
      }
      throw new StaticError(404, "not_found", `${method} ${path} (not part of the static browser profile)`);
    }
    if (method === "POST") {
      if (path === "/api/query") return ok(await query(body as QueryReq, deps, st));
      if (path === "/api/query/capability") {
        // "request -> PlanGraph -> capability evaluation": can this deployment run what the request means? (the UI enables or disables from this, it never guesses)
        const planned = planRequest(body as QueryReq, deps.pack.sources);
        return ok({ supported: planned.missingOps.length === 0, missing_ops: planned.missingOps });
      }
      if (path === "/api/probe") {
        st.probe = body as ProbeReport;
        return ok({ accepted: true, usable: usable(st.probe) });
      }
      if (path === "/api/report") {
        const r = body as { ok?: boolean; message?: string };
        st.events += 1;
        if (r.ok === false) { st.failures += 1; st.lastFailure = String(r.message ?? "failed").slice(0, 200); }
        return ok({ recorded: true });
      }
      if (path === "/api/network") return ok({ profile: (body as { profile?: string }).profile ?? "static" });
      if (path === "/api/ir/run" || path === "/api/ir/explain" || path === "/api/query/plan" || path === "/api/devicecheck") {
        throw new StaticError(422, "server_required", `This analysis requires GeoFabric Server: ${path} is ${NOT_IN_DEPLOYMENT}.`);
      }
    }
    throw new StaticError(404, "not_found", `${method} ${path}`);
  } catch (e) {
    if (e instanceof StaticError) return { status: e.status, body: { error: e.code, message: e.message, ...(e.planCode ? { code: e.planCode } : {}) } };
    return { status: 500, body: { error: "internal", message: e instanceof Error ? e.message : String(e) } };
  }
}

/**
 * Route `fetch("/api/...")` to `handle`. Everything else (the map style, tiles, the published Parquet files) goes to the network
 * untouched. `baseUrl` is the app's base path (`/geofabric/` on GitHub Pages).
 */
export async function installStaticGateway(baseUrl: string): Promise<void> {
  const origin = new URL(baseUrl, location.href);
  const [sources, catalog, manifest] = await Promise.all([
    fetch(new URL("demo/sources.json", origin).href).then((r) => r.json() as Promise<{ sources: StaticSource[] }>),
    fetch(new URL("demo/catalog.json", origin).href).then((r) => r.json()),
    fetch(new URL("demo/pack.json", origin).href).then((r) => r.json() as Promise<DataManifest>),
  ]);
  // one place resolves data URLs: the manifest (a remote, verified host gives absolute URLs; a bundled pack is relative to the page)
  const fileUrl = (id: string) => {
    const f = manifest.files[id];
    return f.url ?? new URL(`demo/${f.path}`, origin).href;
  };
  const deps: GatewayDeps = { pack: { sources: sources.sources, catalog, manifest }, fileUrl };
  const st = newState();
  const real = window.fetch.bind(window);
  window.fetch = (async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const raw = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    const path = raw.startsWith("/api/") ? raw : raw.startsWith(location.origin + "/api/") ? raw.slice(location.origin.length) : null;
    if (!path) return real(input as RequestInfo, init);
    const method = (init?.method ?? (typeof input === "object" && "method" in input ? input.method : "GET")).toUpperCase();
    let body: unknown;
    try { body = init?.body ? JSON.parse(String(init.body)) : undefined; } catch { body = undefined; }
    const reply = await handle(method, path.split("?")[0], body, deps, st);
    return new Response(JSON.stringify(reply.body), { status: reply.status, headers: { "content-type": "application/json" } });
  }) as typeof window.fetch;
}
