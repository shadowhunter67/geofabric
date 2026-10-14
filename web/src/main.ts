import * as maplibregl from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import workerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?url";
import "./style.css";
import { DuckDbWasmEngine } from "./engine/wasm.ts";
import { compile } from "./engine/sqlgen.ts";
import { resultDigest } from "./engine/digest.ts";
import { wkbToGeoJSON } from "./engine/wkb.ts";
import { EngineError } from "./engine/contract.ts";
import type { PlanGraph, SourceBinding } from "./engine/contract.ts";
import { isFresh, runProbe, sessionId, usable } from "./engine/probe.ts";
import type { ProbeReport } from "./engine/probe.ts";
import { classifyFailure, fallbackAllowed } from "./engine/failure.ts";
import type { FailureClass } from "./engine/failure.ts";

// ---------- types ----------
interface Column { name: string; type: string }
interface SourceMeta {
  id: string; format: string; size_bytes: number; rows: number; row_groups: number; crs: string; crs_known: boolean;
  geometry_column: string; geometry_type: string; bbox: [number, number, number, number] | null; has_covering: boolean;
  columns: Column[]; error?: string;
}
interface Decision { rule: string; subject: string; outcome: string; reason: string }
interface Factor { name: string; value: number; weight: number; detail: string }
interface Confidence { score: number; label: "high" | "medium" | "low"; factors: Factor[] }
interface Risk { score: number; memory: number; result_rows: number; probe: number; health: number; format: number; memory_input_bytes: number; crash_threshold_bytes: number; threshold_basis: string }
interface Health { state: "healthy" | "degraded" | "unhealthy" | "unknown"; reason: string; events: number; failures: number; last_failure: string | null }
interface Candidate {
  engine: string; kind: "server" | "browser"; feasible: boolean; score_ms: number | null;
  cost: { fixed_ms: number; scan_ms: number; transfer_ms: number; latency_ms: number; cold_start_ms: number; scan_bytes: number; transfer_bytes: number } | null;
  reasons: string[]; rejected_because: string[];
  confidence: Confidence | null; risk: Risk | null; health: Health; probe_status: string;
}
interface Estimate {
  rows: number; low: number; high: number; quality: "poor" | "fair" | "good"; covering_quality: string; raw_selected: number;
  correction: { factor: number; samples: number; basis: string }; spatial_coherence: number | null; row_groups_total: number; row_groups_kept: number;
  limit_bound: boolean; notes: string[];
}
interface Assessment {
  confidence: Confidence; engine_confidence: number; margin_factor: number; estimate: Estimate;
  network: { name: string; rtt_ms: number; down_mbps: number; up_mbps: number; origin: string };
  health: Record<string, Health>;
  probes: Record<string, { status: string; usable: boolean; age_ms: number | null; failed_checks: string[]; memory_class: string | null; engine_version: string | null; runtime_version: string | null }>;
  fallback: { engine: string; reason: string; max_fallbacks: number } | null; policy: string[];
  browser_access: { uri: string; strategy: "full" | "range"; file_bytes: number; requests_est: number; fetch_bytes_est: number }[];
}
interface Physical {
  engine: string; placement?: "server" | "browser"; decisions: Decision[]; steps: { label: string; detail: string }[];
  assessment: Assessment;
  candidates?: Candidate[]; input_bytes_upper_bound?: number; result_rows_estimate?: number;
  scans: { uri: string; covering?: { xmin: string; ymin: string; xmax: string; ymax: string; declared: boolean } | null;
    prune: { kept: number[]; skipped: number; total_row_groups: number; total_rows: number; kept_rows: number; kept_bytes: number; total_bytes: number } }[];
}
interface Metric<T> { value: T | null; source: string }
interface Metrics {
  elapsed_ms: Metric<number>; rows_returned: Metric<number>; row_groups_total: Metric<number>;
  row_groups_skipped: Metric<number>; rows_scanned_upper_bound: Metric<number>;
  estimated_bytes_upper_bound: Metric<number>; bytes_read: Metric<number>; peak_memory_bytes: Metric<number>;
  notes: string[];
}
interface RunResp {
  fallback?: { from: string; reason: string; class: FailureClass };
  placement?: "server" | "browser"; result_digest?: { count: number; sha256: string };
  source_urls?: Record<string, string>; limits?: { max_rows: number; timeout_ms: number };
  plan_id: string; ir: unknown; physical: Physical; lineage: { id: string; op: string }[];
  columns: { name: string; type: string; geometry: boolean }[]; rows: Record<string, unknown>[]; rows_total: number;
  geojson: GeoJSON.FeatureCollection | null; metrics: Metrics;
  timing: { plan_and_overhead_ms: Metric<number>; total_ms: Metric<number>; boundary_conversion_ms: Metric<number> };
}

// ---------- tiny DOM helper (text only: file contents are untrusted) ----------
type Child = Node | string | null | undefined;
function el<K extends keyof HTMLElementTagNameMap>(tag: K, props: Record<string, unknown> = {}, ...kids: Child[]): HTMLElementTagNameMap[K] {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === "class") n.className = String(v);
    else if (k.startsWith("on") && typeof v === "function") n.addEventListener(k.slice(2), v as EventListener);
    else if (v !== undefined && v !== false) n.setAttribute(k, v === true ? "" : String(v));
  }
  for (const c of kids) if (c != null) n.append(typeof c === "string" ? document.createTextNode(c) : c);
  return n;
}
const fmtBytes = (b: number | null | undefined) => b == null ? "—" : b < 1024 ? `${b} B` : b < 1048576 ? `${(b / 1024).toFixed(1)} KB` : `${(b / 1048576).toFixed(1)} MB`;
const fmtMs = (v: number) => v < 1000 ? `${v.toFixed(1)} ms` : `${(v / 1000).toFixed(2)} s`;

// ---------- state ----------
const S = {
  sources: [] as SourceMeta[],
  cur: null as SourceMeta | null,
  last: null as RunResp | null,
  derived: [] as { label: string; resp: RunResp }[],
  logs: [] as string[],
  tab: "query",
  selected: null as GeoJSON.Feature | null,
  liveRequery: false,
  limit: 20000,
  kind: "features",
  filter: { column: "", op: "eq", value: "" },
  groupBy: "",
  busy: false,
  engine: "auto",
  allowBrowser: true,
  probe: null as ProbeReport | null,
  probing: null as Promise<void> | null,
  network: "LOCAL",
};
const SESSION = sessionId();

// ---------- layout ----------
const app = document.getElementById("app")!;
const statusEl = el("span", { class: "status" }, "idle");
const runBtn = el("button", { class: "primary", onclick: () => void run() }, "Run");
const settingsPop = el("div", { class: "card", style: "position:absolute;right:10px;top:40px;z-index:5;display:none;min-width:240px" });
const header = el("header", {},
  el("div", { class: "brand" }, "Geo", el("span", {}, "Fabric")),
  runBtn,
  el("button", { onclick: () => setTab("profile") }, "Profile"),
  el("button", { onclick: () => { settingsPop.style.display = settingsPop.style.display === "none" ? "block" : "none"; } }, "Settings"),
  el("div", { class: "spacer" }),
  statusEl,
);
const left = el("aside", {});
const mapDiv = el("div", { id: "map" });
const right = el("section", { class: "right" });
const tabsEl = el("div", { class: "tabs" });
const tabBody = el("div", { class: "tabbody" });
const bottom = el("div", { class: "bottom" }, tabsEl, tabBody);
app.append(header, left, mapDiv, right, bottom, settingsPop);

function log(msg: string, isErr = false) {
  S.logs.push(`${new Date().toISOString().slice(11, 23)} ${isErr ? "ERROR " : ""}${msg}`);
  if (S.logs.length > 500) S.logs.shift();
  if (S.tab === "logs") renderTab();
}

// ---------- settings ----------
settingsPop.append(
  el("label", {}, el("input", { type: "checkbox", onchange: (e: Event) => { S.liveRequery = (e.target as HTMLInputElement).checked; } }), " Re-query when the map moves"),
  el("label", { class: "net" }, "Assumed network ",
    el("select", { "data-testid": "network-select", onchange: (e: Event) => void setNetwork((e.target as HTMLSelectElement).value) },
      ...["LOCAL", "FAST_LAN", "GOOD_BROADBAND", "MOBILE", "HIGH_LATENCY"].map((n) => el("option", { value: n }, n)))),
  el("div", { class: "note" }, "What the planner assumes about the link between you and the data (presets are assumptions, not measurements; LOCAL = this host)."),
  el("div", { class: "note" }, "Basemap: none (offline; no external tile requests)."),
  el("div", { class: "note" }, "Mode: Developer (SQL is not exposed; submit logical IR)."),
);

// ---------- map ----------
maplibregl.setWorkerUrl(workerUrl);
const css = getComputedStyle(document.documentElement);
const map = new maplibregl.Map({
  container: mapDiv,
  style: { version: 8, sources: {}, layers: [{ id: "bg", type: "background", paint: { "background-color": css.getPropertyValue("--bg").trim() || "#14181c" } }] },
  center: [105, 14], zoom: 5,
  attributionControl: false,
});
map.addControl(new maplibregl.NavigationControl({ showCompass: false }), "top-left");
map.addControl(new maplibregl.ScaleControl({ unit: "metric" }), "bottom-right");
const empty: GeoJSON.FeatureCollection = { type: "FeatureCollection", features: [] };
map.on("load", () => {
  map.addSource("extent", { type: "geojson", data: empty });
  map.addSource("rowgroups", { type: "geojson", data: empty });
  map.addSource("result", { type: "geojson", data: empty });
  map.addLayer({ id: "rg-fill", type: "fill", source: "rowgroups", paint: { "fill-color": ["match", ["get", "state"], "kept", "#5fbf8a", "skipped", "#6b7782", "#40a9a0"], "fill-opacity": ["match", ["get", "state"], "kept", 0.12, "skipped", 0.04, 0.05] } });
  map.addLayer({ id: "rg-line", type: "line", source: "rowgroups", paint: { "line-color": ["match", ["get", "state"], "kept", "#5fbf8a", "skipped", "#6b7782", "#40a9a0"], "line-width": 1, "line-dasharray": [2, 2] } });
  map.addLayer({ id: "extent-line", type: "line", source: "extent", paint: { "line-color": "#e0a84a", "line-width": 1.5 } });
  map.addLayer({ id: "res-fill", type: "fill", source: "result", filter: ["==", ["geometry-type"], "Polygon"], paint: { "fill-color": "#e0a84a", "fill-opacity": 0.45 } });
  map.addLayer({ id: "res-line", type: "line", source: "result", paint: { "line-color": "#f2c36b", "line-width": 1 } });
  map.addLayer({ id: "res-pt", type: "circle", source: "result", filter: ["==", ["geometry-type"], "Point"], paint: { "circle-radius": 3, "circle-color": "#f2c36b" } });
  map.addLayer({ id: "res-sel", type: "line", source: "result", filter: ["==", ["get", "__i"], -1], paint: { "line-color": "#ffffff", "line-width": 3 } });
  for (const id of ["res-fill", "res-pt"]) {
    map.on("mouseenter", id, () => (map.getCanvas().style.cursor = "pointer"));
    map.on("mouseleave", id, () => (map.getCanvas().style.cursor = ""));
    map.on("click", id, (e) => {
      const f = e.features?.[0];
      if (f) select(f as unknown as GeoJSON.Feature);
    });
  }
  void loadSources();
  void probeRuntime("startup");
});
mapDiv.append(el("div", { class: "legend" },
  el("i", { style: "background:#e0a84a" }), "result  ", el("i", { style: "background:#5fbf8a" }), "row group read  ",
  el("i", { style: "background:#6b7782" }), "row group pruned",
));
let moveTimer: number | undefined;
map.on("moveend", () => {
  if (!S.liveRequery || !S.cur || S.busy) return;
  window.clearTimeout(moveTimer);
  moveTimer = window.setTimeout(() => void run(), 400);
});

function setData(id: string, data: GeoJSON.FeatureCollection) {
  (map.getSource(id) as maplibregl.GeoJSONSource | undefined)?.setData(data);
}

// ---------- api ----------
async function api<T>(path: string, body?: unknown): Promise<T> {
  const t0 = performance.now();
  const r = await fetch(path, body === undefined ? undefined : { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  const j = await r.json();
  log(`${body === undefined ? "GET" : "POST"} ${path} -> ${r.status} (${(performance.now() - t0).toFixed(0)} ms)`);
  if (!r.ok) {
    const issues = Array.isArray(j.issues) ? "\n" + j.issues.map((i: { code: string; message: string }) => `  [${i.code}] ${i.message}`).join("\n") : "";
    throw new Error(`${j.error ?? r.status}: ${j.message ?? ""}${issues}`);
  }
  return j as T;
}

async function loadSources() {
  try {
    S.sources = (await api<{ sources: SourceMeta[] }>("/api/sources")).sources;
    renderLeft();
    const first = S.sources.find((s) => s.id === "clustered_10mb_spatial") ?? S.sources.find((s) => !s.error);
    if (first) await selectSource(first);
  } catch (e) {
    log(String(e), true);
    statusEl.textContent = "gateway unreachable";
  }
}

async function selectSource(s: SourceMeta) {
  S.cur = s;
  S.filter.column = s.columns.find((c) => c.type === "utf8")?.name ?? "";
  S.groupBy = S.filter.column;
  renderLeft();
  if (s.bbox) {
    const [x0, y0, x1, y1] = s.bbox;
    setData("extent", { type: "FeatureCollection", features: [rect(x0, y0, x1, y1, {})] });
    map.fitBounds([[x0, y0], [x1, y1]], { padding: 40, duration: 0 });
  }
  try {
    const rg = await api<{ row_groups: { index: number; bbox: number[] | null }[] }>(`/api/sources/${encodeURIComponent(s.id)}/row_groups`);
    setData("rowgroups", { type: "FeatureCollection", features: rg.row_groups.filter((r) => r.bbox).map((r) => rect(r.bbox![0], r.bbox![1], r.bbox![2], r.bbox![3], { index: r.index, state: "unknown" })) });
  } catch (e) { log(String(e), true); }
  renderTab();
  await run();
}

function rect(x0: number, y0: number, x1: number, y1: number, props: Record<string, unknown>): GeoJSON.Feature {
  return { type: "Feature", properties: props, geometry: { type: "Polygon", coordinates: [[[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]]] } };
}

// ---------- run ----------
function currentRequest() {
  const b = map.getBounds();
  const bbox = [Math.max(-180, b.getWest()), Math.max(-90, b.getSouth()), Math.min(180, b.getEast()), Math.min(90, b.getNorth())];
  const f = S.filter;
  return {
    source: S.cur!.id, kind: S.kind, bbox, crs: S.cur!.crs, limit: S.limit,
    filter: f.column && f.value !== "" ? { column: f.column, op: f.op, value: isNaN(Number(f.value)) ? f.value : Number(f.value) } : undefined,
    group_by: S.kind === "count_by" ? S.groupBy : undefined,
    engine: S.engine === "auto" ? undefined : S.engine,
    browser: S.allowBrowser,
    client: { warm: wasm.isWarm() },
  };
}


// ---------- engine placement: the planner decides, the browser engine runs only when chosen ----------
const wasm = new DuckDbWasmEngine();

function bindSources(r: RunResp): Record<string, SourceBinding> {
  const out: Record<string, SourceBinding> = {};
  for (const [uri, url] of Object.entries(r.source_urls ?? {})) {
    const scan = r.physical.scans.find((x) => x.uri === uri);
    out[uri] = { url, covering: scan?.covering ?? null, access: r.physical.assessment?.browser_access?.find((a) => a.uri === uri)?.strategy ?? "range" };
  }
  return out;
}

// ---------- runtime probe, reports (ADR-009/010) ----------
/** Probe the browser engine and tell the planner. Re-runs on reload (new session), TTL expiry and after a fatal failure. */
function probeRuntime(reason: string): Promise<void> {
  if (S.probing) return S.probing;
  S.probing = (async () => {
    try {
      const report = await runProbe(wasm, SESSION);
      S.probe = report;
      await api("/api/probe", report);
      log(`runtime probe (${reason}): ${usable(report) ? "usable" : "NOT usable"}, ${report.checks.filter((c) => c.ok).length}/${report.checks.length} checks, ${report.duration_ms.toFixed(0)} ms, memory class ${report.memory_class}`);
    } catch (e) {
      log(`runtime probe failed: ${e}`, true);
    } finally {
      S.probing = null;
      if (S.tab === "execution") renderTab();
    }
  })();
  return S.probing;
}

async function setNetwork(name: string) {
  try {
    await api("/api/network", { profile: name });
    S.network = name;
    log(`planner network profile: ${name}`);
  } catch (e) { log(String(e), true); }
}

/** Fire-and-forget: tell the gateway how a browser run went (health, known incompatibilities, telemetry). */
function reportRun(r: RunResp, ok: boolean, rowsOrErr: number | string, ms: number, fallback = false) {
  const scan = r.physical.scans[0];
  const source = scan ? scan.uri.split(/[\\/]/).pop()!.replace(/\.parquet$/, "") : "";
  const est = r.physical.assessment?.estimate;
  const ir = r.ir as PlanGraph;
  const nodes = Object.values(ir.nodes);
  const agg = nodes.find((n) => n.op === "aggregate");
  const lim = nodes.filter((n) => n.op === "limit").map((n) => (n.op === "limit" ? n.n : Infinity));
  const groupBy = agg && agg.op === "aggregate" && agg.group_by.length ? agg.group_by[0] : undefined;
  const body = ok
    ? {
        engine: r.physical.engine, source, ok: true, kind: groupBy ? "count_by" : agg ? "count" : "features", raw_selected: est?.raw_selected ?? 0,
        total_rows: r.physical.scans.reduce((a, x) => a + (x.prune?.total_rows ?? 0), 0), est_rows: r.physical.result_rows_estimate ?? 0,
        actual_rows: rowsOrErr as number, limit: lim.length ? Math.min(...lim) : undefined, group_by: groupBy,
        est_ms: r.physical.candidates?.find((c) => c.engine === r.physical.engine)?.score_ms ?? undefined, actual_ms: ms, fallback,
      }
    : { engine: r.physical.engine, source, ok: false, message: String(rowsOrErr) };
  void fetch("/api/report", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }).catch(() => {});
}

/**
 * Browser execution can fail for reasons the planner cannot see from metadata (measured: DuckDB-Wasm
 * cannot read natively-typed Parquet GEOMETRY columns written by DuckDB >= 1.5, e.g. an Overture
 * subset; very large inputs crash the tab). The failure is never hidden: the same request is re-planned
 * with the browser disabled, and the response carries `fallback` so the UI says what happened.
 */
async function placeWithFallback(first: RunResp, retry: () => Promise<RunResp>): Promise<RunResp> {
  const t0 = performance.now();
  try {
    const res = await resolvePlacement(first);
    if (first.placement === "browser") reportRun(first, true, res.rows_total, performance.now() - t0);
    return res;
  } catch (e) {
    const reason = e instanceof Error ? e.message : String(e);
    const cls = classifyFailure(reason);
    reportRun(first, false, reason, performance.now() - t0);
    if (!fallbackAllowed(cls)) throw e; // cancelled by the user: no fallback
    log(`browser engine failed (${cls}), falling back once to the server engine: ${reason}`, true);
    await wasm.recycle();
    // a crash/out-of-memory means the runtime we probed is gone: probe the new instance (in the background)
    if (cls === "ENGINE_CRASH" || cls === "MEMORY") { S.probe = null; void probeRuntime(`after ${cls}`); }
    const second = await retry();
    return { ...second, fallback: { from: first.physical.engine, reason, class: cls } };
  }
}

/** If the planner placed the plan in the browser, run it there and return a server-shaped response. */
async function resolvePlacement(r: RunResp): Promise<RunResp> {
  if (r.placement !== "browser") return r;
  const t0 = performance.now();
  const plan = r.ir as PlanGraph;
  let res;
  try {
    res = await wasm.execute(plan, { sources: bindSources(r), timeoutMs: r.limits?.timeout_ms ?? 30000, maxRows: r.limits?.max_rows ?? 100000, estimatedInputBytes: r.physical.input_bytes_upper_bound });
  } catch (e) {
    const k = e instanceof EngineError ? `${e.kind}: ` : "";
    throw new Error(`browser engine failed (${k}${e instanceof Error ? e.message : String(e)})`);
  }
  const t1 = performance.now();
  const srcNode = Object.values(plan.nodes).find((n) => n.op === "source");
  const geomName = srcNode && srcNode.op === "source" ? srcNode.schema.geometry_columns[0]?.name : undefined;
  const geomIdx = res.columns.findIndex((c) => c.name === geomName);
  const scalarCols = res.columns.map((c, i) => ({ c, i })).filter(({ i }) => {
    const v = res.rows.find((row) => row[i] !== null && row[i] !== undefined)?.[i];
    return i !== geomIdx && !(v instanceof Uint8Array) && !(typeof v === "object" && v !== null);
  });
  const cell = (v: unknown) => (typeof v === "bigint" ? (Number.isSafeInteger(Number(v)) ? Number(v) : v.toString()) : v);
  const objects = res.rows.map((row) => Object.fromEntries(scalarCols.map(({ c, i }) => [c.name, cell(row[i])])));
  const features = geomIdx >= 0
    ? res.rows.map((row, k) => ({ type: "Feature" as const, properties: objects[k], geometry: row[geomIdx] instanceof Uint8Array ? wkbToGeoJSON(row[geomIdx] as Uint8Array) : null }))
    : null;
  const digest = await resultDigest(res.rows, res.columns.map((c) => c.type));
  const t2 = performance.now();
  const sum = (f: (s: Physical["scans"][number]) => number) => r.physical.scans.reduce((a, s) => a + f(s), 0);
  const m = (value: number | null, source: string): Metric<number> => ({ value, source });
  const metrics: Metrics = {
    elapsed_ms: res.metrics.elapsed_ms, rows_returned: res.metrics.rows_returned,
    row_groups_total: m(sum((s) => s.prune.total_row_groups), "source_observed"),
    row_groups_skipped: m(sum((s) => s.prune.skipped), "source_estimated"),
    rows_scanned_upper_bound: m(sum((s) => s.prune.kept_rows), "source_estimated"),
    estimated_bytes_upper_bound: m(sum((s) => s.prune.kept_bytes), "source_estimated"),
    bytes_read: res.metrics.bytes_read, peak_memory_bytes: m(null, "unknown"),
    notes: ["executed in the browser (DuckDB-Wasm); the server only planned and served authorized Parquet byte ranges", ...res.metrics.notes],
  };
  return {
    ...r,
    columns: res.columns.map((c, i) => ({ name: c.name, type: c.type, geometry: i === geomIdx })),
    rows: objects.slice(0, 1000), rows_total: res.rows.length,
    geojson: features ? { type: "FeatureCollection", features: features.filter((f) => f.geometry) as GeoJSON.Feature[] } : null,
    metrics, result_digest: digest,
    timing: {
      total_ms: m(t2 - t0, "engine_observed"),
      plan_and_overhead_ms: m(Math.max(0, t1 - t0 - (res.metrics.elapsed_ms.value ?? 0)), "derived"),
      boundary_conversion_ms: m(t2 - t1, "engine_observed"),
    },
  };
}

async function run() {
  if (!S.cur || S.busy) return;
  if (!["EPSG:4326", "OGC:CRS84"].includes(S.cur.crs)) {
    log(`viewport queries need a lon/lat source (EPSG:4326 or OGC:CRS84) in v0; source CRS: ${S.cur.crs}`, true);
    statusEl.textContent = "unsupported CRS";
    return;
  }
  if (!S.probing && !isFresh(S.probe)) void probeRuntime(S.probe ? "ttl expired" : "no probe yet");
  S.busy = true; runBtn.disabled = true; statusEl.textContent = "running…";
  try {
    const req = currentRequest();
    await handle(
      await placeWithFallback(await api<RunResp>("/api/query", req), () => api<RunResp>("/api/query", { ...req, engine: "duckdb", browser: false })),
      `${S.kind} @ ${S.cur.id}`,
    );
  } catch (e) {
    log(String(e), true); statusEl.textContent = "error";
    setTab("logs");
  } finally { S.busy = false; runBtn.disabled = false; }
}

async function handle(r: RunResp, label: string, record = true) {
  S.last = r;
  if (record) {
    S.derived.unshift({ label, resp: r });
    S.derived = S.derived.slice(0, 8);
  }
  S.selected = null;
  if (r.geojson) {
    r.geojson.features.forEach((f, i) => { (f.properties ??= {}).__i = i; });
    setData("result", r.geojson);
  } else setData("result", empty);
  const kept = new Set(r.physical.scans[0]?.prune.kept ?? []);
  const rg = (map.getSource("rowgroups") as maplibregl.GeoJSONSource | undefined);
  if (rg) {
    const data = await rg.getData() as GeoJSON.FeatureCollection;
    for (const f of data.features) f.properties!.state = kept.has(f.properties!.index as number) ? "kept" : "skipped";
    rg.setData(data);
  }
  statusEl.textContent = `${r.rows_total} rows · ${fmtMs(r.timing.total_ms.value ?? 0)} · ${r.physical.engine}`;
  log(`plan ${r.plan_id.slice(0, 10)} engine=${r.physical.engine} rows=${r.rows_total} skipped(est.)=${r.metrics.row_groups_skipped.value}/${r.metrics.row_groups_total.value}`);
  renderLeft(); renderRight();
  setTab(S.tab === "query" ? "execution" : S.tab);
}

// ---------- inspector ----------
const R = 6371008.8;
const rad = (d: number) => (d * Math.PI) / 180;
function ringArea(r: number[][]) { let s = 0; for (let i = 0; i < r.length - 1; i++) s += rad(r[i + 1][0] - r[i][0]) * (2 + Math.sin(rad(r[i][1])) + Math.sin(rad(r[i + 1][1]))); return Math.abs((s * R * R) / 2); }
function ringLen(r: number[][]) { let s = 0; for (let i = 0; i < r.length - 1; i++) { const dl = rad(r[i + 1][0] - r[i][0]), dp = rad(r[i + 1][1] - r[i][1]); const a = Math.sin(dp / 2) ** 2 + Math.cos(rad(r[i][1])) * Math.cos(rad(r[i + 1][1])) * Math.sin(dl / 2) ** 2; s += 2 * R * Math.asin(Math.min(1, Math.sqrt(a))); } return s; }
function measure(g: GeoJSON.Geometry): { area: number; perim: number } | null {
  const polys = g.type === "Polygon" ? [g.coordinates] : g.type === "MultiPolygon" ? g.coordinates : null;
  if (!polys) return null;
  let area = 0, perim = 0;
  for (const p of polys) p.forEach((ring, i) => { area += (i === 0 ? 1 : -1) * ringArea(ring); perim += ringLen(ring); });
  return { area, perim };
}
function select(f: GeoJSON.Feature) {
  S.selected = f;
  map.setFilter("res-sel", ["==", ["get", "__i"], (f.properties?.__i as number) ?? -1]);
  renderRight();
}

function renderRight() {
  right.replaceChildren(el("h3", {}, "Inspector"));
  const f = S.selected;
  if (!f) { right.append(el("div", { class: "empty" }, "Click a feature on the map.")); }
  else {
    const t = el("table", { class: "kv" });
    const add = (k: string, v: string) => t.append(el("tr", {}, el("td", {}, k), el("td", {}, v)));
    for (const [k, v] of Object.entries(f.properties ?? {})) if (k !== "__i") add(k, String(v));
    add("geometry type", f.geometry.type);
    add("source", S.cur?.id ?? "—");
    add("CRS", S.cur?.crs ?? "—");
    const m = ["EPSG:4326", "OGC:CRS84"].includes(S.cur?.crs ?? "") ? measure(f.geometry) : null;
    if (m) { add("area ≈", `${m.area.toFixed(1)} m²`); add("perimeter ≈", `${m.perim.toFixed(1)} m`); }
    right.append(el("h3", {}, "Properties"), t);
    if (m) right.append(el("div", { class: "note", style: "padding:0 10px" }, `≈ spherical model, WGS84 sphere R=${R} m (not ellipsoidal).`));
  }
  right.append(el("h3", {}, "Lineage"));
  if (S.last) {
    const ol = el("div", { style: "padding:0 10px 10px" });
    S.last.lineage.forEach((l, i) => ol.append(el("div", { class: "dec" }, `${i === 0 ? "" : "↓ "}${l.op}  `, el("span", { style: "color:var(--muted)" }, l.id))));
    ol.append(el("div", { class: "note" }, `plan ${S.last.plan_id.slice(0, 12)}`));
    right.append(ol);
  } else right.append(el("div", { class: "empty" }, "No query yet."));
}

// ---------- left ----------
function renderLeft() {
  left.replaceChildren(el("h3", {}, "Data / Layers"), el("h3", {}, "Sources"));
  if (!S.sources.length) left.append(el("div", { class: "empty" }, "No GeoParquet files in the data directory."));
  for (const s of S.sources) {
    if (s.error) { left.append(el("div", { class: "item" }, el("div", { class: "name" }, s.id), el("div", { class: "meta" }, s.error))); continue; }
    left.append(el("div", { class: `item${S.cur?.id === s.id ? " sel" : ""}`, tabindex: 0, onclick: () => void selectSource(s) },
      el("div", { class: "name" }, s.id),
      el("div", { class: "meta" }, `${s.format} · ${s.geometry_type} · ${s.crs}${s.crs_known ? "" : " (unknown!)"}`),
      el("div", { class: "meta" }, `${fmtBytes(s.size_bytes)} · ${s.rows.toLocaleString()} rows · ${s.row_groups} rg`),
    ));
  }
  left.append(el("h3", {}, "Derived"));
  if (!S.derived.length) left.append(el("div", { class: "empty" }, "Results of queries appear here."));
  S.derived.forEach((d, i) => left.append(el("div", { class: `item${i === 0 ? " sel" : ""}`, onclick: () => void handle(d.resp, d.label, false) },
    el("div", { class: "name" }, d.label),
    el("div", { class: "meta" }, `${d.resp.rows_total} rows · ${fmtMs(d.resp.timing.total_ms.value ?? 0)}`),
  )));
}

// ---------- tabs ----------
const TABS: [string, string][] = [["query", "SQL / Ask"], ["results", "Results"], ["execution", "Execution Plan"], ["profile", "Profile"], ["logs", "Logs"]];
function setTab(t: string) { S.tab = t; renderTab(); }
function renderTab() {
  tabsEl.replaceChildren(...TABS.map(([id, label]) => el("div", { class: `tab${S.tab === id ? " on" : ""}`, tabindex: 0, role: "tab", onclick: () => setTab(id), onkeydown: (e: KeyboardEvent) => { if (e.key === "Enter") setTab(id); } }, label)));
  tabBody.replaceChildren();
  if (S.tab === "query") tabBody.append(queryTab());
  else if (S.tab === "results") tabBody.append(resultsTab());
  else if (S.tab === "execution") tabBody.append(executionTab());
  else if (S.tab === "profile") tabBody.append(profileTab());
  else tabBody.append(el("div", { class: "log" }, ...S.logs.map((l) => el("div", { class: l.includes(" ERROR ") ? "err" : "" }, l))));
}

function queryTab(): HTMLElement {
  const cols = S.cur?.columns.filter((c) => c.type !== "other" && typeof c.type === "string") ?? [];
  const sel = (opts: [string, string][], val: string, on: (v: string) => void) =>
    el("select", { onchange: (e: Event) => on((e.target as HTMLSelectElement).value) }, ...opts.map(([v, l]) => el("option", { value: v, selected: v === val }, l)));
  const colOpts: [string, string][] = [["", "(none)"], ...cols.map((c): [string, string] => [c.name, c.name])];
  const form = el("div", { class: "form" },
    el("label", {}, "Query", sel([["features", "Features in viewport (bbox)"], ["intersects_features", "Features intersecting viewport polygon"], ["count_by", "Count by column in viewport"]], S.kind, (v) => { S.kind = v; renderTab(); })),
    el("label", {}, "Filter column", sel(colOpts, S.filter.column, (v) => { S.filter.column = v; })),
    el("label", {}, "op", sel([["eq", "="], ["ne", "≠"], ["gt", ">"], ["ge", "≥"], ["lt", "<"], ["le", "≤"]], S.filter.op, (v) => { S.filter.op = v; })),
    el("label", {}, "value", el("input", { value: S.filter.value, oninput: (e: Event) => { S.filter.value = (e.target as HTMLInputElement).value; } })),
    S.kind === "count_by" ? el("label", {}, "group by", sel(colOpts.slice(1), S.groupBy, (v) => { S.groupBy = v; })) : null,
    el("label", {}, "feature limit", el("input", { type: "number", min: 1, max: 100000, value: S.limit, onchange: (e: Event) => { S.limit = Math.min(100000, Math.max(1, Number((e.target as HTMLInputElement).value) || 20000)); } })),
    el("label", {}, "Engine", sel([["auto", "Auto (planner decides)"], ["duckdb", "Server / DuckDB"], ["duckdb-wasm", "Browser / DuckDB-Wasm"]], S.engine, (v) => { S.engine = v; })),
    el("label", { style: "flex-direction:row;align-items:center;gap:4px" }, el("input", { type: "checkbox", checked: S.allowBrowser, onchange: (e: Event) => { S.allowBrowser = (e.target as HTMLInputElement).checked; } }), "allow browser execution"),
    el("button", { class: "primary", onclick: () => void run() }, "Run viewport query"),
  );
  const ta = el("textarea", { class: "ir", spellcheck: false, "aria-label": "Spatial IR JSON" }) as HTMLTextAreaElement;
  ta.value = S.last ? JSON.stringify(S.last.ir, null, 2) : "";
  const irBar = el("div", { class: "form", style: "margin-top:8px" },
    el("button", { onclick: () => void runIr(ta.value, false) }, "Run IR"),
    el("button", { onclick: () => void runIr(ta.value, true) }, "Explain IR (no execution)"),
    el("span", { class: "note" }, "Logical IR only: engine-neutral, CRS and units explicit. Raw SQL is intentionally not accepted."),
  );
  return el("div", {}, form, el("div", { class: "note" }, "Viewport = current map extent in the source CRS. Each run is compiled to logical IR (below), validated, planned, then executed."), ta, irBar);
}

async function runIr(text: string, explainOnly: boolean) {
  let ir: unknown;
  try { ir = JSON.parse(text); } catch (e) { log(`IR is not valid JSON: ${e}`, true); setTab("logs"); return; }
  try {
    if (explainOnly) {
      const r = await api<{ physical: Physical }>("/api/ir/explain", { ir });
      S.last = { ...(S.last as RunResp), physical: r.physical, metrics: { ...(S.last?.metrics as Metrics) } };
      setTab("execution");
    } else {
      const body = { ir, engine: S.engine === "auto" ? undefined : S.engine, browser: S.allowBrowser };
      await handle(await placeWithFallback(await api<RunResp>("/api/ir/run", body), () => api<RunResp>("/api/ir/run", { ir, engine: "duckdb", browser: false })), "IR run");
    }
  } catch (e) { log(String(e), true); setTab("logs"); }
}

function resultsTab(): HTMLElement {
  const r = S.last;
  if (!r) return el("div", { class: "empty" }, "Run a query.");
  const cols = r.columns.filter((c) => !c.geometry && !c.type.startsWith("Struct"));
  const t = el("table", {}, el("thead", {}, el("tr", {}, ...cols.map((c) => el("th", {}, c.name)))));
  const tb = el("tbody", {});
  for (const row of r.rows) tb.append(el("tr", {}, ...cols.map((c) => el("td", {}, String(row[c.name] ?? "∅")))));
  t.append(tb);
  return el("div", {}, el("div", { class: "note", style: "margin:0 0 6px" }, `${r.rows_total} rows returned${r.rows.length < r.rows_total ? ` (table shows first ${r.rows.length}; map shows all)` : ""}. Geometry columns are drawn on the map.`), t);
}

const BADGE: Record<string, [string, string]> = {
  engine_observed: ["Observed", "ok"], source_observed: ["Observed", "ok"],
  source_estimated: ["Estimated", "est"], planner_estimated: ["Estimated", "est"],
  derived: ["Derived", "der"], unknown: ["Unavailable", "na"],
};
/** A metric card: value + provenance badge. Estimates are never shown as observed facts. */
function mcard(label: string, m: Metric<number>, fmt: (v: number) => string, prefix = ""): HTMLElement {
  const [text, cls] = m.value == null ? BADGE.unknown : (BADGE[m.source] ?? BADGE.unknown);
  return el("div", { class: "card", "data-metric": label, "data-source": m.value == null ? "unknown" : m.source },
    el("div", { class: "v" }, m.value == null ? "Unknown" : prefix + fmt(m.value)),
    el("div", { class: "l" }, label, " ", el("span", { class: `badge ${cls}` }, text)));
}
const fmtN = (v: number) => v.toLocaleString();

function metricCards(m: Metrics, tm: RunResp["timing"]): HTMLElement {
  const skipped = m.row_groups_skipped.value, total = m.row_groups_total.value;
  const rg: Metric<number> = { value: skipped, source: m.row_groups_skipped.source };
  const cards = [
    mcard("Total (plan + execute + boundary)", tm.total_ms, fmtMs),
    mcard("Engine execution", m.elapsed_ms, fmtMs),
    mcard("Rows returned", m.rows_returned, fmtN),
    mcard("Row groups total", m.row_groups_total, fmtN),
    mcard(`Row groups skipped (of ${total ?? "?"}; from GeoParquet statistics)`, rg, fmtN),
    mcard("Rows scanned, upper bound", m.rows_scanned_upper_bound, fmtN, "≤ "),
    mcard("Bytes read, upper bound", m.estimated_bytes_upper_bound, fmtBytesN, "≤ "),
    mcard("Bytes read (actual I/O)", m.bytes_read, fmtBytesN),
    mcard("Peak buffer memory (engine)", m.peak_memory_bytes, fmtBytesN),
  ];
  return el("div", { class: "cards" }, ...cards);
}
const fmtBytesN = (v: number) => fmtBytes(v);

function executionTab(): HTMLElement {
  const r = S.last;
  if (!r) return el("div", { class: "empty" }, "Run a query to see how it executes.");
  const p = r.physical;
  const pipe = el("div", { class: "pipe" });
  const steps = [...p.steps.map((s) => ({ t: s.label, d: s.detail })),
    { t: "Render", d: `${r.rows_total} rows → table / map (GeoJSON boundary conversion ${fmtMs(r.timing.boundary_conversion_ms.value ?? 0)})` }];
  steps.forEach((s, i) => {
    if (i) pipe.append(el("div", { class: "arrow" }, "↓"));
    pipe.append(el("div", { class: "step" }, el("div", { class: "t" }, s.t), el("div", { class: "d" }, s.d.replace("\\\\?\\", ""))));
  });
  const dec = el("div", {}, el("h3", { style: "padding-left:0" }, "Planner decisions"),
    ...p.decisions.map((d) => el("div", { class: "dec" }, el("span", { class: `o-${d.outcome}` }, `[${d.outcome}] `), `${d.rule} · ${d.subject}: ${d.reason}`)));
  return el("div", {}, plannerPanel(r), metricCards(r.metrics, r.timing), pipe, dec, ...r.metrics.notes.map((n) => el("div", { class: "note" }, n)));
}

const ENGINE_LABEL: Record<string, string> = { "duckdb-wasm": "Local Browser / DuckDB-Wasm", duckdb: "Server / DuckDB" };

/** Which engine runs the plan, what the alternatives were, and why. */
function plannerPanel(r: RunResp): HTMLElement {
  const p = r.physical;
  const sel = p.candidates?.find((c) => c.engine === p.engine);
  const alts = (p.candidates ?? []).filter((c) => c.engine !== p.engine);
  const where = p.placement === "browser" ? "Local (browser)" : "Server";
  const inputMb = p.input_bytes_upper_bound == null ? "—" : `≤ ${fmtBytes(p.input_bytes_upper_bound)}`;
  const transfer = sel?.cost ? fmtBytes(sel.cost.transfer_bytes) : "—";
  const box = el("div", { class: "planner", "data-testid": "planner-panel" },
    el("div", { class: "row" },
      el("span", { class: `engine ${p.placement ?? "server"}`, "data-testid": "engine-selected", "data-engine": p.engine }, ENGINE_LABEL[p.engine] ?? p.engine),
      el("span", { class: "badge der" }, where),
      sel?.score_ms != null ? el("span", { class: "note" }, `score ${sel.score_ms.toFixed(1)} ms (lower wins)`) : null),
    el("div", { class: "kv2" },
      el("span", {}, "Estimated input"), el("b", {}, inputMb),
      el("span", {}, p.placement === "browser" ? "Estimated transfer to the browser" : "Estimated result transfer"), el("b", {}, `~${transfer}`)),
    ...(sel?.reasons ?? []).map((t) => el("div", { class: "dec" }, "✓ ", t)),
    ...alts.map((c) => el("div", { class: "alt", "data-testid": "engine-alternative", "data-engine": c.engine },
      el("b", {}, `Alternative: ${ENGINE_LABEL[c.engine] ?? c.engine}`), "  ",
      c.feasible ? `score ${c.score_ms?.toFixed(1)} ms` : "not feasible", c.feasible ? "" : ` — ${c.rejected_because.join("; ")}`)),
    r.fallback ? el("div", { class: "dec", "data-testid": "engine-fallback" }, el("span", { class: "o-warning" }, "[fallback] "), `the browser engine failed (${r.fallback.class ?? "?"}: ${r.fallback.reason.slice(0, 160)}); the query was re-run once on the server`) : null,
    r.result_digest ? el("div", { class: "note" }, `result digest ${r.result_digest.sha256.slice(0, 12)} · count ${r.result_digest.count}`) : null,
    assessmentPanel(r));
  return box;
}

const pct = (v: number) => `${Math.round(v * 100)}%`;
function badge(text: string, cls: string, testid?: string): HTMLElement {
  return el("span", { class: `badge ${cls}`, "data-testid": testid }, text);
}
const HEALTH_BADGE: Record<string, [string, string]> = { healthy: ["Healthy", "ok"], degraded: ["Degraded", "warn"], unhealthy: ["Unhealthy", "bad"], unknown: ["Unknown", "na"] };

/** Planner self-assessment: confidence, health, probe, network, risk and fallback (ADR-009/010). */
function assessmentPanel(r: RunResp): HTMLElement {
  const p = r.physical, a = p.assessment;
  if (!a) return el("div", {});
  const sel = p.candidates?.find((c) => c.engine === p.engine);
  const browser = p.candidates?.find((c) => c.kind === "browser");
  const probe = a.probes?.["duckdb-wasm"];
  const kv = (k: string, ...v: (Child | Child[])[]) => [el("span", {}, k), el("b", {}, ...v.flat())] as HTMLElement[];
  const conf = a.confidence;
  const confCls = conf.label === "high" ? "ok" : conf.label === "medium" ? "warn" : "bad";
  const est = a.estimate;
  const actual = r.rows_total;
  const factor = (e: number, t: number) => { const x = (e + 1) / (t + 1); return Math.max(x, 1 / x); };
  const rows: HTMLElement[] = [
    ...kv("Engine", ENGINE_LABEL[p.engine] ?? p.engine, " ", badge(p.placement === "browser" ? "Local" : "Server", "der")),
    ...kv("Confidence", badge(`${conf.label === "low" ? "Low Confidence" : conf.label === "medium" ? "Medium" : "High"} ${pct(conf.score)}`, confCls, "confidence-badge"),
      el("span", { class: "note", "data-testid": "confidence-factors", title: conf.factors.map((f) => `${f.name} ${f.value.toFixed(2)} (w ${f.weight})`).join("\n") },
        `  engine ${pct(a.engine_confidence)} × margin ${a.margin_factor.toFixed(2)}`)),
    ...kv("Health", ...Object.entries(a.health ?? {}).flatMap(([e, h]) => [badge(`${e}: ${HEALTH_BADGE[h.state]?.[0] ?? h.state}`, HEALTH_BADGE[h.state]?.[1] ?? "na", `health-${e}`), " "])),
    ...kv("Runtime probe", probe ? [badge(probe.status === "fresh" && probe.usable ? "Observed" : probe.status === "missing" ? "Not probed" : probe.status === "stale" ? "Stale" : "Failed", probe.status === "fresh" && probe.usable ? "ok" : "warn", "probe-badge"),
      el("span", { class: "note" }, probe.age_ms != null ? `  ${(probe.age_ms / 1000).toFixed(0)} s ago · ${probe.memory_class ?? "?"} memory · wasm ${probe.engine_version ?? "?"}${probe.failed_checks.length ? ` · not OK: ${probe.failed_checks.join(", ")}` : ""}` : "  static description only")] : "—"),
    ...kv("Network profile", `${a.network.name} `, el("span", { class: "note" }, `(rtt ${a.network.rtt_ms} ms · ${a.network.down_mbps} Mbps · ${a.network.origin})`)),
    ...kv("Result estimate", badge("Estimated", "est"), ` ~${est.rows.toLocaleString()} rows (${est.low.toLocaleString()}–${est.high.toLocaleString()}), quality ${est.quality}`,
      r.rows_total != null ? [" · ", badge("Observed", "ok"), el("span", { "data-testid": "estimate-vs-actual" }, ` ${actual.toLocaleString()} rows (×${factor(est.rows, actual).toFixed(1)} off)`)] : []),
    ...kv("Browser risk", browser?.risk ? [`${pct(browser.risk.score)} `, el("span", { class: "note", "data-testid": "risk" }, `memory ${pct(Math.min(browser.risk.memory, 9.99))} of the crash threshold (${fmtBytes(browser.risk.memory_input_bytes)} / ${fmtBytes(browser.risk.crash_threshold_bytes)})`)] : browser && !browser.feasible ? `rejected — ${browser.rejected_because[0]?.slice(0, 120) ?? ""}` : "—"),
    ...kv("Fallback", a.fallback ? `${ENGINE_LABEL[a.fallback.engine] ?? a.fallback.engine} (at most ${a.fallback.max_fallbacks}, no retry on the same engine)` : p.placement === "browser" ? "none feasible" : "not needed (server plan)"),
  ];
  void sel;
  return el("div", { class: "kv2 assess", "data-testid": "assessment" }, ...rows,
    ...(a.policy ?? []).map((t) => el("div", { class: "dec", style: "grid-column:1/-1" }, el("span", { class: "o-warning" }, "[policy] "), t)));
}

function profileTab(): HTMLElement {
  const r = S.last;
  if (!r) return el("div", { class: "empty" }, "No profile yet.");
  return el("div", {}, metricCards(r.metrics, r.timing),
    el("table", {}, el("tbody", {},
      el("tr", {}, el("td", {}, "gateway planning + overhead"), el("td", {}, fmtMs(r.timing.plan_and_overhead_ms.value ?? 0))),
      el("tr", {}, el("td", {}, "engine execution"), el("td", {}, fmtMs(r.metrics.elapsed_ms.value ?? 0))),
      el("tr", {}, el("td", {}, "GeoJSON boundary conversion"), el("td", {}, fmtMs(r.timing.boundary_conversion_ms.value ?? 0))))),
    el("div", { class: "note" }, "CPU / network profiling is not implemented in the MVP; only the numbers above are measured."));
}

renderLeft(); renderRight(); renderTab();

// Test hook for the browser smoke test only (?e2e=1): exposes the map and state, nothing else.
if (new URLSearchParams(location.search).has("e2e")) {
  (window as unknown as Record<string, unknown>).__gf = { map, state: S, select, setTab, wasm, resultDigest, resolvePlacement, bindSources, probeRuntime, setNetwork, placeWithFallback, reportRun, compile };
}
