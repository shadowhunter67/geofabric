import * as maplibregl from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import workerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?url";
import "./style.css";
import { compile } from "./engine/sqlgen.ts";
import type { PlanGraph } from "./engine/contract.ts";
import { usable } from "./engine/probe.ts";
import { createPlacement } from "./placement.ts";
import { getLang, LANGS, onLangChange, setLang, t } from "./ui/i18n.ts";
import type { Key, Lang } from "./ui/i18n.ts";
import { datasetLabel, isPrimaryDataset, fieldLabel, fmtBytes, fmtDate, fmtInt, fmtMs, fmtPct, geometryLabel } from "./ui/labels.ts";
import { pipelineSteps, summarize } from "./ui/explain.ts";
import { icon } from "./ui/icons.ts";
import { extentOf, planFocus } from "./focus.ts";
import { fetchCatalog, datasetsByGroup, sourceIdOf, attributionText } from "./catalog.ts";
import type { Catalog, CatalogDataset } from "./catalog.ts";
import { classFilter, layerSpecs, loadLayerStates, renderPolicy, saveLayerStates } from "./layers.ts";
import type { LayerState } from "./layers.ts";
import { inspectorRows } from "./inspector-schema.ts";
import { lengthOf } from "./measure.ts";
import { loadDem } from "./raster.ts";
import { BUILDINGS, DEM, DEMOS, NEAR_DISTANCE_M, PLACES, demoRequests, splitPlaces, topGroup } from "./demos.ts";
import type { DemoSpec } from "./demos.ts";
import type { DemOverlay } from "./raster.ts";
import type { Assessment, Candidate, Health, Metric, Metrics, Physical, RunResp, SourceMeta } from "./types.ts";

// ---------- tiny DOM helper (text only: file contents are untrusted) ----------
type Child = Node | string | null | undefined | Child[];
function el<K extends keyof HTMLElementTagNameMap>(tag: K, props: Record<string, unknown> = {}, ...kids: Child[]): HTMLElementTagNameMap[K] {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === "class") n.className = String(v);
    else if (k.startsWith("on") && typeof v === "function") n.addEventListener(k.slice(2), v as EventListener);
    else if (k.startsWith("aria-") && typeof v === "boolean") n.setAttribute(k, String(v)); // ARIA states are "true"/"false" strings, never empty
    else if (v !== undefined && v !== false && v !== null) n.setAttribute(k, v === true ? "" : String(v));
  }
  const add = (c: Child): void => {
    if (c == null) return;
    if (Array.isArray(c)) c.forEach(add);
    else n.append(typeof c === "string" ? document.createTextNode(c) : c);
  };
  kids.forEach(add);
  return n;
}

/** Replace the children of `n` (null/undefined/nested arrays allowed, like `el`). */
function fill(n: HTMLElement, ...kids: Child[]): void {
  n.replaceChildren();
  const add = (c: Child): void => {
    if (c == null) return;
    if (Array.isArray(c)) c.forEach(add);
    else n.append(typeof c === "string" ? document.createTextNode(c) : c);
  };
  kids.forEach(add);
}

// ---------- settings (persisted per viewer; every access is guarded: storage may be blocked) ----------
type Mode = "explorer" | "developer";
const params = new URLSearchParams(location.search);
function stored(k: string): string | null {
  try { return localStorage.getItem(k); } catch { return null; }
}
function store(k: string, v: string): void {
  try { localStorage.setItem(k, v); } catch { /* blocked */ }
}
const initialMode: Mode = (params.get("mode") ?? stored("gf.mode")) === "developer" ? "developer" : "explorer";
const initialLang: Lang = ((params.get("lang") ?? stored("gf.lang")) === "en" ? "en" : "vi");
setLang(initialLang);

// ---------- state ----------
const S = {
  sources: [] as SourceMeta[],
  catalog: null as Catalog | null,
  cur: null as SourceMeta | null,
  layers: loadLayerStates() as Record<string, LayerState>,
  demo: { id: null as string | null, summary: "" },
  contextData: new Map<string, GeoJSON.FeatureCollection>(),
  raster: new Map<string, { loading: boolean; error: string | null; overlay: DemOverlay | null }>(),
  context: new Map<string, { features: number; minZoom: number; limitHit: boolean; visible: boolean; loading: boolean; seq: number }>(),
  last: null as RunResp | null,
  lastChosen: false,
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
  settling: false, // true while a dataset is being opened (first query, then the focus query)
  overlays: stored("gf.overlays") === "1", // developer-only debug layers (row groups, dataset extent); never shown in Explorer
  basemap: stored("gf.basemap") !== "0",
  engine: "auto",
  allowBrowser: true,
  network: "LOCAL",
  mode: initialMode,
  expanded: new Set<string>(),
  advancedOpen: false,
  status: { key: "status.ready" as Key, params: {} as Record<string, string | number> },
};
const DEV = () => S.mode === "developer";

const placement = createPlacement({
  log: (m, e) => log(m, e),
  onProbe: () => { if (S.tab === "execution") renderTab(); if (DEV()) renderHeader(); },
});
const wasm = placement.wasm;

// ---------- layout ----------
const app = document.getElementById("app")!;
const statusEl = el("span", { class: "status", role: "status", "aria-live": "polite", "data-testid": "status" });
const header = el("header", { role: "banner" });
const left = el("aside", { class: "left" });
const mapDiv = el("div", { id: "map", role: "region" });
const right = el("section", { class: "right" });
const tabsEl = el("div", { class: "tabs", role: "tablist" });
const tabBody = el("div", { class: "tabbody", role: "tabpanel", tabindex: "0" });
const pipeEl = el("div", { class: "pipeline", "data-testid": "pipeline" });
const resizer = el("div", { class: "resizer", role: "separator", "aria-orientation": "horizontal", tabindex: "0" });
const bottom = el("div", { class: "bottom", role: "region" }, resizer, tabsEl, tabBody, pipeEl);
const legendEl = el("div", { class: "legend" });
const popover = el("div", { class: "popover", hidden: true, role: "dialog" });
app.append(header, left, mapDiv, right, bottom, popover);
const attributionEl = el("div", { class: "map-attribution", tabindex: "0" });
const demoCardEl = el("div", { class: "demo-card", role: "status", "data-testid": "demo-summary", hidden: true });
mapDiv.append(legendEl, attributionEl, demoCardEl);

function log(msg: string, isErr = false) {
  S.logs.push(`${new Date().toISOString().slice(11, 23)} ${isErr ? "ERROR " : ""}${msg}`);
  if (S.logs.length > 500) S.logs.shift();
  if (S.tab === "logs") renderTab();
}

function setStatus(key: Key, p: Record<string, string | number> = {}) {
  S.status = { key, params: p };
  renderStatus();
}
function renderStatus() {
  statusEl.textContent = t(S.status.key, S.status.params);
}

// ---------- bottom panel height (drag or keyboard) ----------
const root = document.documentElement;
let bottomH = Number(stored("gf.bottomH")) || 0;
function applyBottomH(h?: number) {
  const max = Math.round(window.innerHeight * 0.55);
  const dflt = Math.min(300, Math.round(window.innerHeight * 0.36));
  bottomH = Math.max(170, Math.min(max, h ?? (bottomH || dflt)));
  root.style.setProperty("--bottom-h", `${bottomH}px`);
  requestAnimationFrame(() => map?.resize());
}
resizer.addEventListener("pointerdown", (e) => {
  e.preventDefault();
  const move = (ev: PointerEvent) => applyBottomH(window.innerHeight - ev.clientY);
  const up = () => { window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", up); store("gf.bottomH", String(bottomH)); };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
});
resizer.addEventListener("keydown", (e) => {
  if (e.key === "ArrowUp" || e.key === "ArrowDown") {
    e.preventDefault();
    applyBottomH(bottomH + (e.key === "ArrowUp" ? 24 : -24));
    store("gf.bottomH", String(bottomH));
  }
});
window.addEventListener("resize", () => applyBottomH());

// ---------- header ----------
function renderHeader() {
  const modeBtn = (m: Mode) =>
    el("button", { type: "button", class: `mode${S.mode === m ? " on" : ""}`, "aria-pressed": S.mode === m, "data-testid": `mode-${m}`, onclick: () => setMode(m) }, t(m === "explorer" ? "mode.explorer" : "mode.developer"));
  const langSel = el("select", { "aria-label": t("lang.label"), title: t("lang.label"), "data-testid": "lang", onchange: (e: Event) => changeLang((e.target as HTMLSelectElement).value as Lang) },
    ...LANGS.map((l) => el("option", { value: l, selected: l === getLang() }, l === "vi" ? "Tiếng Việt" : "English")));
  const runBtn = el("button", { type: "button", class: "primary", id: "run", "data-testid": "run", title: t("top.run"), onclick: () => void run() }, t("top.run"));
  runBtn.disabled = S.busy;
  const open = (k: string) => !popover.hidden && popover.dataset.kind === k;
  const settingsBtn = el("button", { type: "button", "aria-haspopup": "dialog", "aria-expanded": open("settings"), "data-testid": "settings", title: t("top.settings"), onclick: () => togglePopover("settings") }, t("top.settings"));
  const helpBtn = el("button", { type: "button", "aria-haspopup": "dialog", "aria-expanded": open("help"), "data-testid": "help", title: t("top.help"), onclick: () => togglePopover("help") }, t("top.help"));
  fill(header,
    el("div", { class: "brand" }, "Geo", el("span", {}, "Fabric")),
    el("div", { class: "modes", role: "group", "aria-label": t("mode.label") }, modeBtn("explorer"), modeBtn("developer")),
    langSel,
    runBtn,
    settingsBtn,
    helpBtn,
    el("div", { class: "spacer" }),
    DEV() ? devStrip() : null,
    statusEl,
  );
  renderStatus();
}

/** Developer-only strip: the engine, the network the planner assumes and the runtime probe. */
function devStrip(): HTMLElement {
  const pr = placement.state.probe;
  return el("div", { class: "devstrip", "data-testid": "dev-strip" },
    el("span", {}, `${t("top.engine")}: ${S.engine === "auto" ? "auto" : S.engine}`),
    el("span", {}, `${t("top.network")}: ${S.network}`),
    el("span", {}, `${t("top.probe")}: ${pr ? (usable(pr) ? "OK" : "FAILED") : placement.state.probing ? "…" : "—"}`));
}

function setMode(m: Mode) {
  if (S.mode === m) return;
  S.mode = m;
  store("gf.mode", m);
  if (!DEV() && S.tab === "profile") S.tab = "query";
  renderAll();
}
function changeLang(l: Lang) {
  store("gf.lang", l);
  setLang(l); // onLangChange re-renders everything
}
onLangChange(() => { document.documentElement.lang = getLang(); renderAll(); });

function togglePopover(kind: "settings" | "help") {
  if (!popover.hidden && popover.dataset.kind === kind) { closePopover(); return; }
  popover.dataset.kind = kind;
  popover.hidden = false;
  renderPopover();
  renderHeader();
}
function closePopover() { popover.hidden = true; delete popover.dataset.kind; renderHeader(); }
window.addEventListener("keydown", (e) => { if (e.key === "Escape" && !popover.hidden) closePopover(); });

function renderPopover() {
  if (popover.hidden) return;
  const kind = popover.dataset.kind;
  popover.setAttribute("aria-label", t(kind === "help" ? "help.title" : "settings.title"));
  if (kind === "help") {
    popover.replaceChildren(el("h3", {}, t("help.title")), el("div", { class: "help-body" }, t("help.body")), el("button", { type: "button", onclick: closePopover }, t("help.close")));
    return;
  }
  fill(popover,
    el("h3", {}, t("settings.title")),
    el("label", { class: "check" }, el("input", { type: "checkbox", checked: S.liveRequery, onchange: (e: Event) => { S.liveRequery = (e.target as HTMLInputElement).checked; } }), t("settings.live")),
    DEV() ? el("label", { class: "net" }, t("settings.network"),
      el("select", { "data-testid": "network-select", onchange: (e: Event) => void setNetwork((e.target as HTMLSelectElement).value) },
        ...["LOCAL", "FAST_LAN", "GOOD_BROADBAND", "MOBILE", "HIGH_LATENCY"].map((n) => el("option", { value: n, selected: n === S.network }, n)))) : null,
    DEV() ? el("div", { class: "note" }, t("settings.network_note")) : null,
    el("label", { class: "check" }, el("input", { type: "checkbox", "data-testid": "basemap-toggle", checked: S.basemap, onchange: (e: Event) => { S.basemap = (e.target as HTMLInputElement).checked; store("gf.basemap", S.basemap ? "1" : "0"); applyMapVisibility(); } }), t("settings.basemap")),
    el("div", { class: "note" }, t("settings.basemap_note")),
    DEV() ? el("label", { class: "check" }, el("input", { type: "checkbox", "data-testid": "overlays-toggle", checked: S.overlays, onchange: (e: Event) => { S.overlays = (e.target as HTMLInputElement).checked; store("gf.overlays", S.overlays ? "1" : "0"); applyMapVisibility(); renderLegend(); } }), t("settings.overlays")) : null,
    el("div", { class: "note" }, t("settings.mode_note")),
    el("button", { type: "button", onclick: closePopover }, t("help.close")),
  );
}

async function setNetwork(name: string) {
  try {
    await api("/api/network", { profile: name });
    S.network = name;
    log(`planner network profile: ${name}`);
    renderHeader();
  } catch (e) { log(String(e), true); }
}

// ---------- map ----------
// Layer order, bottom to top: basemap (roads, water, areas) -> debug overlays (Developer only, off by default) -> result features
// -> selected feature -> basemap place names. The debug layers show what the engine read; they are never the result and never above it.
maplibregl.setWorkerUrl(workerUrl);
const css = getComputedStyle(document.documentElement);
/** OpenFreeMap: vector basemap without an API key. Fetched once; offline or blocked -> the map works on a plain background. */
const BASEMAP_STYLE = "https://tiles.openfreemap.org/styles/dark";
const BG_STYLE: maplibregl.StyleSpecification = { version: 8, sources: {}, layers: [{ id: "bg", type: "background", paint: { "background-color": css.getPropertyValue("--bg").trim() || "#14181c" } }] };
const map = new maplibregl.Map({ container: mapDiv, style: BG_STYLE, center: [105, 14], zoom: 5, attributionControl: false });
map.addControl(new maplibregl.AttributionControl({ compact: true }), "bottom-left");
map.addControl(new maplibregl.NavigationControl({ showCompass: false }), "top-left");
map.addControl(new maplibregl.ScaleControl({ unit: "metric" }), "bottom-right");
const empty: GeoJSON.FeatureCollection = { type: "FeatureCollection", features: [] };
const DEBUG_LAYERS = ["rg-fill", "rg-line", "extent-line"];
let baseLayerIds: string[] = [];
let basemapLoaded = false;
function applyMapVisibility() {
  const vis = (on: boolean) => (on ? "visible" : "none");
  for (const id of baseLayerIds) if (map.getLayer(id)) map.setLayoutProperty(id, "visibility", vis(S.basemap));
  for (const id of DEBUG_LAYERS) if (map.getLayer(id)) map.setLayoutProperty(id, "visibility", vis(DEV() && S.overlays));
}
async function installBasemap(): Promise<void> {
  try {
    const ctl = new AbortController();
    const timer = window.setTimeout(() => ctl.abort(), 6000);
    const r = await fetch(BASEMAP_STYLE, { signal: ctl.signal });
    window.clearTimeout(timer);
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    const style = (await r.json()) as maplibregl.StyleSpecification;
    await new Promise<void>((done) => { map.once("style.load", () => done()); map.setStyle(style); });
    baseLayerIds = (map.getStyle().layers ?? []).filter((l) => l.type !== "background").map((l) => l.id);
    basemapLoaded = true;
  } catch (e) {
    log(`basemap unavailable (${String(e)}); continuing on a plain background`, true);
  }
}
function addResultLayers() {
  // result layers go below the first place-name (symbol) layer of the basemap, so names stay readable above the features
  // Anchor = the first label (symbol) layer that comes AFTER the last geometry layer of the basemap. Basemaps interleave symbol
  // layers with roads and buildings (OpenFreeMap puts a water label before them), so "the first symbol layer" would bury our
  // layers under the basemap's own roads and buildings.
  const basemapLayers = map.getStyle().layers ?? [];
  const lastGeometry = basemapLayers.reduce((n, l, i) => (l.type !== "symbol" && l.type !== "background" ? i : n), -1);
  const before = basemapLayers.find((l, i) => i > lastGeometry && l.type === "symbol")?.id;
  map.addSource("extent", { type: "geojson", data: empty });
  map.addSource("rowgroups", { type: "geojson", data: empty });
  map.addSource("result", { type: "geojson", data: empty });
  const add = (l: maplibregl.LayerSpecification) => map.addLayer(l, before);
  // debug overlays (below the result)
  add({ id: "rg-fill", type: "fill", source: "rowgroups", layout: { visibility: "none" }, paint: { "fill-color": ["match", ["get", "state"], "kept", "#5fbf8a", "skipped", "#6b7782", "#40a9a0"], "fill-opacity": ["match", ["get", "state"], "kept", 0.1, "skipped", 0.04, 0.05] } });
  add({ id: "rg-line", type: "line", source: "rowgroups", layout: { visibility: "none" }, paint: { "line-color": ["match", ["get", "state"], "kept", "#5fbf8a", "skipped", "#6b7782", "#40a9a0"], "line-width": 1, "line-dasharray": [2, 2] } });
  add({ id: "extent-line", type: "line", source: "extent", layout: { visibility: "none" }, paint: { "line-color": "#8a96a3", "line-width": 1, "line-dasharray": [4, 3] } });
  // result features: the actual geometries
  add({ id: "res-fill", type: "fill", source: "result", filter: ["any", ["==", ["geometry-type"], "Polygon"], ["==", ["geometry-type"], "MultiPolygon"]], paint: { "fill-color": "#4aa3ff", "fill-opacity": 0.5 } });
  add({ id: "res-line", type: "line", source: "result", paint: { "line-color": "#8cc4ff", "line-width": 1 } });
  add({ id: "res-pt", type: "circle", source: "result", filter: ["==", ["geometry-type"], "Point"], paint: { "circle-radius": 4, "circle-color": "#4aa3ff", "circle-stroke-color": "#0e1318", "circle-stroke-width": 1 } });
  // selected feature, above every result layer
  map.addSource("selected", { type: "geojson", data: empty });
  add({ id: "res-sel-fill", type: "fill", source: "selected", filter: ["any", ["==", ["geometry-type"], "Polygon"], ["==", ["geometry-type"], "MultiPolygon"]], paint: { "fill-color": "#ffd34a", "fill-opacity": 0.55 } });
  add({ id: "res-sel", type: "line", source: "selected", paint: { "line-color": "#ffffff", "line-width": 3 } });
  add({ id: "res-sel-pt", type: "circle", source: "selected", filter: ["==", ["geometry-type"], "Point"], paint: { "circle-radius": 8, "circle-color": "#ffd34a", "circle-stroke-color": "#ffffff", "circle-stroke-width": 2 } });
}
map.on("load", async () => {
  await installBasemap();
  addResultLayers();
  applyMapVisibility();
  map.on("click", (e) => {
    // a few pixels of tolerance so thin lines and small points can be hit; the top-most rendered feature wins
    const layers = clickableLayers();
    if (!layers.length) return;
    const box: [maplibregl.PointLike, maplibregl.PointLike] = [[e.point.x - 5, e.point.y - 5], [e.point.x + 5, e.point.y + 5]];
    const hit = map.queryRenderedFeatures(box, { layers })[0];
    if (hit) select(originalFeature(hit as unknown as GeoJSON.Feature));
  });
  map.on("mousemove", (e) => {
    const layers = clickableLayers();
    const over = layers.length > 0 && map.queryRenderedFeatures([[e.point.x - 5, e.point.y - 5], [e.point.x + 5, e.point.y + 5]], { layers }).length > 0;
    map.getCanvas().style.cursor = over ? "pointer" : "";
  });
  void loadSources();
  void placement.probeRuntime("startup");
});
function renderLegend() {
  const showDebug = DEV() && S.overlays;
  legendEl.replaceChildren(
    el("i", { style: "background:#4aa3ff" }), t("map.legend.result"),
    ...(showDebug ? ["  ", el("i", { style: "background:#5fbf8a" }), t("map.legend.read_dev"), "  ", el("i", { style: "background:#6b7782" }), t("map.legend.skipped_dev")] : []),
    ...(S.selected ? ["  ", el("i", { style: "background:#ffd34a" }), t("map.legend.selected")] : []));
}
let moveTimer: number | undefined;
map.on("moveend", () => {
  window.clearTimeout(moveTimer);
  moveTimer = window.setTimeout(() => {
    if (S.liveRequery && S.cur && !S.busy) void run();
    void refreshContext();
  }, 400);
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
  setStatus("status.loading");
  try {
    const [sources, catalog] = await Promise.all([api<{ sources: SourceMeta[] }>("/api/sources"), fetchCatalog()]);
    S.sources = sources.sources;
    S.catalog = catalog;
    renderLeft();
    // real buildings over the real basemap when the Overture subset is present; else the small synthetic file
    const first = S.sources.find((s) => s.id === params.get("source") && !s.error) ?? S.sources.find((s) => s.id === "overture_buildings_vn_south" && !s.error) ?? S.sources.find((s) => s.id === "clustered_10mb_spatial") ?? S.sources.find((s) => !s.error);
    if (first) await selectSource(first);
  } catch (e) {
    log(String(e), true);
    setStatus("status.unreachable");
  }
}

async function selectSource(s: SourceMeta) {
  S.settling = true;
  try { await openSource(s); } finally { S.settling = false; }
}
async function openSource(s: SourceMeta) {
  S.cur = s;
  S.filter.column = s.columns.find((c) => c.type === "utf8")?.name ?? "";
  S.groupBy = S.filter.column;
  renderLeft();
  if (s.bbox) {
    const [x0, y0, x1, y1] = s.bbox;
    setData("extent", { type: "FeatureCollection", features: [rect(x0, y0, x1, y1, {})] }); // debug overlay only
    map.fitBounds([[x0, y0], [x1, y1]], { padding: 40, duration: 0 });
  }
  try {
    const rg = await api<{ row_groups: { index: number; bbox: number[] | null }[] }>(`/api/sources/${encodeURIComponent(s.id)}/row_groups`);
    setData("rowgroups", { type: "FeatureCollection", features: rg.row_groups.filter((r) => r.bbox).map((r) => rect(r.bbox![0], r.bbox![1], r.bbox![2], r.bbox![3], { index: r.index, state: "unknown" })) });
  } catch (e) { log(String(e), true); }
  renderTab();
  await run();
  await focusOnResult();
}

/**
 * First view of a dataset: fitted to the whole dataset its features are sub-pixel. If the result cannot be seen at this zoom, go to the
 * densest area at a zoom where a typical feature is several pixels wide, and run the query again for what is in view.
 */
async function focusOnResult() {
  const fc = S.last?.geojson;
  if (!fc || S.busy) return;
  const f = planFocus(fc, { zoom: map.getZoom() });
  if (!f) return;
  log(`result features are ~${f.medianPx.toFixed(2)} px at zoom ${map.getZoom().toFixed(1)}; focusing on the densest area (zoom ${f.zoom.toFixed(1)})`);
  setStatus("status.focusing");
  await new Promise<void>((done) => { map.once("moveend", () => done()); map.jumpTo({ center: f.center, zoom: f.zoom }); });
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

async function run() {
  if (!S.cur || S.busy) return;
  if (!["EPSG:4326", "OGC:CRS84"].includes(S.cur.crs)) {
    log(`viewport queries need a lon/lat source (EPSG:4326 or OGC:CRS84) in v0; source CRS: ${S.cur.crs}`, true);
    setStatus("status.unsupported_crs");
    return;
  }
  placement.ensureFreshProbe();
  S.busy = true; setRunDisabled(true); setStatus("status.running");
  try {
    const req = currentRequest();
    S.lastChosen = S.engine !== "auto";
    await handle(
      await placement.placeWithFallback(await apiRetryBusy<RunResp>("/api/query", req), () => apiRetryBusy<RunResp>("/api/query", { ...req, engine: "duckdb", browser: false })),
      `${S.kind} @ ${S.cur.id}`,
    );
  } catch (e) {
    log(String(e), true); setStatus("status.error");
    setTab("logs");
  } finally { S.busy = false; setRunDisabled(false); }
}
function setRunDisabled(v: boolean) {
  const b = document.getElementById("run") as HTMLButtonElement | null;
  if (b) b.disabled = v;
}

async function handle(r: RunResp, label: string, record = true) {
  S.last = r;
  if (record) {
    S.derived.unshift({ label, resp: r });
    S.derived = S.derived.slice(0, 8);
  }
  S.selected = null;
  (map.getSource("selected") as maplibregl.GeoJSONSource | undefined)?.setData(empty);
  setStatus("status.drawing");
  if (r.geojson) {
    r.geojson.features.forEach((f, i) => { (f.properties ??= {}).__i = i; (f.properties ??= {}).__layer = S.cur?.id ?? ""; });
    setData("result", r.geojson);
  } else setData("result", empty);
  const kept = new Set(r.physical.scans[0]?.prune.kept ?? []);
  const rg = (map.getSource("rowgroups") as maplibregl.GeoJSONSource | undefined);
  if (rg) {
    const data = await rg.getData() as GeoJSON.FeatureCollection;
    for (const f of data.features) f.properties!.state = kept.has(f.properties!.index as number) ? "kept" : "skipped";
    rg.setData(data);
  }
  setStatus(DEV() ? "status.results_dev" : "status.results", { n: fmtInt(r.rows_total), time: fmtMs(r.timing.total_ms.value ?? 0), engine: r.physical.engine });
  log(t("log.plan", { plan: r.plan_id.slice(0, 10), engine: r.physical.engine, rows: r.rows_total }) + ` skipped(est.)=${r.metrics.row_groups_skipped.value}/${r.metrics.row_groups_total.value}`);
  syncInspectorVisibility();
  renderLeft(); renderRight(); renderPipeline();
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
/** Layers a click can select: the active result and every catalog context layer (not labels, not debug overlays, not the selection). */
function clickableLayers(): string[] {
  return (map.getStyle().layers ?? [])
    .map((l) => l.id)
    .filter((id) => ["res-fill", "res-pt"].includes(id) || (id.startsWith("ctx-") && !id.endsWith("-label")));
}
/** Rendered features are clipped and simplified per tile; the inspector measures the ORIGINAL geometry from the loaded data. */
function originalFeature(f: GeoJSON.Feature): GeoJSON.Feature {
  const layer = String(f.properties?.__layer ?? "");
  const index = Number(f.properties?.__i);
  const fc = layer && layer !== S.cur?.id ? S.contextData.get(layer) : S.last?.geojson;
  const original = fc?.features[index];
  return original && original.properties?.__i === index ? original : f;
}
function select(f: GeoJSON.Feature) {
  S.selected = f;
  (map.getSource("selected") as maplibregl.GeoJSONSource | undefined)?.setData({ type: "FeatureCollection", features: [f] });
  renderLegend();
  syncInspectorVisibility();
  renderRight();
}
/** The inspector takes no room until there is something to inspect. */
function syncInspectorVisibility() {
  const show = S.selected != null;
  if (app.classList.contains("no-inspector") === show) {
    app.classList.toggle("no-inspector", !show);
    requestAnimationFrame(() => map.resize());
  }
}

function renderRight() {
  right.setAttribute("aria-label", t("a11y.inspector"));
  right.replaceChildren(el("h2", {}, t("inspector.title")));
  const f = S.selected;
  if (!f) { right.append(el("div", { class: "empty" }, t("inspector.empty"))); return; }
  const tbl = el("table", { class: "kv", "data-testid": "inspector-props" });
  const add = (k: string, v: string, raw?: string) =>
    tbl.append(el("tr", {}, el("th", { scope: "row" }, k, DEV() && raw ? el("div", { class: "raw" }, t("inspector.raw_field", { name: raw })) : null), el("td", {}, v)));
  // Type-specific rows for catalog layers (buildings, roads, boundaries, places, land use); other sources keep the generic list.
  // Explorer shows the facts that exist; empty attributes (null) are noise there, Developer still lists them.
  const dataset = S.catalog?.datasets.find((d) => d.id === f.properties?.__layer) ?? null;
  const crs = dataset?.crs ?? S.cur?.crs ?? "";
  const m = ["EPSG:4326", "OGC:CRS84"].includes(crs) ? measure(f.geometry) : null;
  if (dataset) {
    const rows = inspectorRows(dataset.group, f.properties ?? {}, {
      lang: getLang(), developer: DEV(), datasetTitle: dataset.title[getLang()], area: m?.area, perimeter: m?.perim, length: lengthOf(f.geometry),
    });
    for (const r of rows) add(r.label, r.value, r.raw);
  } else {
    for (const [k, v] of Object.entries(f.properties ?? {})) {
      if (k !== "__i" && k !== "__layer" && (DEV() || (v !== null && v !== undefined && v !== ""))) add(fieldLabel(k), typeof v === "boolean" ? t(v ? "value.yes" : "value.no") : String(v), k);
    }
    add(t("inspector.source"), S.cur ? datasetLabel(S.cur.id).title : "—");
    if (m) { add(t("inspector.area"), `${fmtInt(Math.round(m.area * 10) / 10)} m²`); add(t("inspector.perimeter"), `${fmtInt(Math.round(m.perim * 10) / 10)} m`); }
  }
  add(t("inspector.geometry"), geometryLabel(f.geometry.type));
  if (DEV()) add(t("inspector.crs"), crs || "—");
  right.append(el("h3", {}, t("inspector.properties")), tbl);
  if (m) right.append(el("div", { class: "note" }, t("inspector.sphere_note", { r: R })));
  right.append(el("h3", {}, t("inspector.history")));
  if (S.last) {
    const steps = pipelineSteps(S.last.ir as PlanGraph, S.last.physical.placement === "browser" ? "browser" : "server").slice(0, -1);
    right.append(el("ol", { class: "history", "data-testid": "history" }, ...steps.map((s) => el("li", {}, t(s.title)))));
    if (DEV()) right.append(el("div", { class: "note" }, `plan ${S.last.plan_id.slice(0, 12)} · ` + S.last.lineage.map((l) => `${l.op} ${l.id}`).join(" → ")));
  } else right.append(el("div", { class: "empty" }, t("inspector.no_history")));
}

// ---------- left: data ----------
function row(k: string, v: string) { return el("tr", {}, el("th", { scope: "row" }, k), el("td", {}, v)); }
function renderSourceItem(s: SourceMeta): HTMLElement {
  if (s.error) return el("div", { class: "item" }, el("div", { class: "name" }, s.id), el("div", { class: "meta" }, s.error));
  const lab = datasetLabel(s.id);
  const open = S.expanded.has(s.id);
  const traits = [...(s.geometry_type === "unknown" ? [] : [geometryLabel(s.geometry_type === "polygon" ? "Polygon" : s.geometry_type)]), fmtBytes(s.size_bytes), ...(DEV() ? lab.traits : [])];
  const detail = el("table", { class: "kv detail", "data-testid": "dataset-detail" },
    row(t("data.format"), s.format), row(t("data.crs"), s.crs), row(t("data.rows"), fmtInt(s.rows)), row(t("data.updated"), fmtDate(s.modified)), row(t("data.origin"), lab.origin),
    ...(DEV() ? [row(t("data.row_groups"), String(s.row_groups)), row(t("data.covering"), s.has_covering ? t(s.covering_inferred ? "data.covering_inferred" : "data.covering_declared") : t("data.covering_none")), row("id", s.id)] : []));
  return el("div", { class: `item${S.cur?.id === s.id ? " sel" : ""}`, role: "listitem", "data-testid": "dataset", "data-id": s.id },
    el("button", { type: "button", class: "pick", "aria-current": S.cur?.id === s.id ? "true" : null, onclick: () => void selectSource(s) },
      el("span", { class: "name" }, lab.title),
      el("span", { class: "meta" }, traits.join(" · ")),
      DEV() ? el("span", { class: "meta raw" }, `${s.id} · ${fmtInt(s.rows)} · ${s.row_groups} rg · ${s.crs}`) : null),
    el("button", { type: "button", class: "more", "aria-expanded": open, title: t(open ? "data.hide_details" : "data.details"), onclick: () => { if (open) S.expanded.delete(s.id); else S.expanded.add(s.id); renderLeft(); } },
      icon("chevron", 14), el("span", {}, t(open ? "data.hide_details" : "data.details"))),
    open ? detail : null);
}

function renderRecent(): HTMLElement {
  return S.derived.length
    ? el("div", { class: "list", role: "list" }, ...S.derived.map((d, i) => el("div", { class: `item${i === 0 ? " sel" : ""}`, role: "listitem" },
        el("button", { type: "button", class: "pick", onclick: () => void handle(d.resp, d.label, false) },
          el("span", { class: "name" }, DEV() ? d.label : datasetLabel(d.label.split("@ ")[1] ?? "").title),
          el("span", { class: "meta" }, t("status.results", { n: fmtInt(d.resp.rows_total), time: fmtMs(d.resp.timing.total_ms.value ?? 0) }))))))
    : el("div", { class: "empty" }, t("data.recent_empty"));
}

function renderLeft() {
  left.setAttribute("aria-label", t("a11y.sources"));
  const list = el("div", { class: "list", role: "list" });
  if (S.catalog) {
    // product catalog: groups of friendly datasets (Explorer); Developer adds the benchmark files that are not part of the pack
    for (const { group, datasets } of datasetsByGroup(S.catalog, { developer: DEV() })) {
      list.append(el("h3", { class: "catalog-group" }, t(`catalog.group.${group}` as Key)));
      for (const d of datasets) list.append(renderCatalogDataset(d));
    }
    if (!list.children.length) list.append(el("div", { class: "empty" }, t("catalog.empty")));
    if (DEV()) {
      const claimed = new Set(S.catalog.datasets.flatMap((d) => d.source_ids));
      const bench = S.sources.filter((s) => !claimed.has(s.id)).slice().sort((a, b) => (a.size_bytes ?? 0) - (b.size_bytes ?? 0));
      if (bench.length) {
        list.append(el("h3", { class: "catalog-group" }, t("catalog.group.benchmark" as Key)));
        for (const s of bench) list.append(renderSourceItem(s));
      }
    }
    left.replaceChildren(el("h2", {}, t("data.title")), list, el("h3", {}, t("data.recent")), renderRecent());
    list.querySelector(".item.sel")?.scrollIntoView?.({ block: "nearest" });
    return;
  }
  if (!S.sources.length) list.append(el("div", { class: "empty" }, t("data.none")));
  // Explorer lists the datasets a person would pick; benchmark variants (shuffled, other draws) are for Developer mode
  const shown = (DEV() ? S.sources : S.sources.filter((s) => isPrimaryDataset(s.id) || s.id === S.cur?.id || s.error)).slice().sort((a, b) => (a.size_bytes ?? 0) - (b.size_bytes ?? 0));
  for (const s of shown) list.append(renderSourceItem(s));
  left.replaceChildren(el("h2", {}, t("data.title")), el("h3", {}, t("data.sources")), list, el("h3", {}, t("data.recent")), renderRecent());
  list.querySelector(".item.sel")?.scrollIntoView?.({ block: "nearest" });
}

function rasterStatus(d: CatalogDataset): string {
  const r = S.raster.get(d.id);
  if (!r || r.loading) return t("catalog.raster_loading");
  if (r.error) return t("catalog.raster_error");
  return r.overlay ? t("catalog.raster_range", { min: fmtInt(Math.round(r.overlay.min)), max: fmtInt(Math.round(r.overlay.max)) }) : "";
}

function renderCatalogDataset(d: CatalogDataset): HTMLElement {
  const source = sourceIdOf(d);
  const isRaster = d.group === "raster" && !!d.raster?.length;
  const state = S.layers[d.id] ?? { visible: S.cur?.id === source, opacity: isRaster ? 0.65 : 1 };
  const info = S.context.get(d.id);
  const zoom = map.getZoom(), policy = renderPolicy(d.group, zoom);
  const toggle = (e: Event) => {
    S.layers[d.id] = { ...state, visible: (e.target as HTMLInputElement).checked };
    saveLayerStates(S.layers); void refreshContext(); renderLeft();
  };
  const opacity = (e: Event) => {
    S.layers[d.id] = { ...state, opacity: Number((e.target as HTMLInputElement).value) / 100 };
    saveLayerStates(S.layers);
    if (isRaster) { if (map.getLayer(`dem-${d.id}`)) map.setPaintProperty(`dem-${d.id}`, "raster-opacity", S.layers[d.id].opacity); } else restyleContext(d);
  };
  const details = S.expanded.has(d.id);
  const fileRows = d.files.reduce((n, f) => n + (f.rows ?? 0), 0);
  return el("div", { class: `item${S.cur?.id === source ? " sel" : ""}`, "data-testid": "dataset", "data-id": d.id },
    el("button", { type: "button", class: "pick", onclick: () => { const found = S.sources.find((s) => s.id === source); if (found) void selectSource(found); } },
      el("span", { class: "name" }, d.title[getLang()]),
      el("span", { class: "meta" }, `${geometryLabel(d.geometry_type)} · ${fmtBytes(d.files.reduce((n, f) => n + f.size_bytes, 0))}`)),
    (source || isRaster) ? el("div", { class: "layer-controls" },
      el("label", { class: "check" }, el("input", { type: "checkbox", checked: state.visible, "aria-label": t("catalog.layer_visible", { name: d.title[getLang()] }), onchange: toggle }), t("catalog.visible")),
      el("input", { type: "range", min: "0", max: "100", step: "10", value: String(state.opacity * 100), "aria-label": t("catalog.opacity", { name: d.title[getLang()] }), "aria-valuetext": `${state.opacity * 100}%`, onchange: opacity }),
      el("button", { type: "button", title: t("catalog.zoom_to"), onclick: () => map.fitBounds(d.coverage.bbox, { padding: 40, maxZoom: 14, duration: 0 }) }, t("catalog.zoom_to")),
      el("span", { class: "meta layer-status", role: "status" }, !state.visible ? "" : isRaster ? rasterStatus(d) : policy.hint ? t("catalog.zoom_in") : info?.loading ? t("catalog.loading") : info?.limitHit ? t("catalog.limit_hit", { n: policy.limit }) : info ? t("catalog.features", { n: fmtInt(info.features) }) : "")) : null,
    el("button", { type: "button", class: "more", "aria-expanded": details, onclick: () => { details ? S.expanded.delete(d.id) : S.expanded.add(d.id); renderLeft(); } }, t(details ? "data.hide_details" : "data.details")),
    details ? el("table", { class: "kv detail", "data-testid": "dataset-detail" },
      row(t("catalog.provider"), d.provider), row(t("catalog.license"), d.license.name), d.license.attribution ? row(t("catalog.attribution"), d.license.attribution) : null,
      row(t("catalog.coverage"), d.coverage.name), row(t("data.rows"), fmtInt(fileRows)), row(t("catalog.size"), fmtBytes(d.files.reduce((n, f) => n + f.size_bytes, 0))),
      DEV() ? row(t("data.crs"), d.crs) : null,
      d.quality ? row(t("catalog.quality"), t("catalog.quality_value", { rows: d.quality.rows ?? 0, nulls: d.quality.null_geometry_count ?? 0, invalid: d.quality.invalid_geometry_count ?? 0 })) : null) : null);
}
function removeContext(id: string) {
  S.contextData.delete(id);
  for (const l of map.getStyle().layers ?? []) if (l.id.startsWith(`ctx-${id}-`)) map.removeLayer(l.id);
  if (map.getSource(`ctx-${id}`)) map.removeSource(`ctx-${id}`);
}
/** Draw order of the catalog groups, bottom to top (the active result and the selection always sit above all of them). */
const STACK = ["raster", "thematic", "admin", "buildings", "transport", "places"];
/** Layer id to insert a group's layers BEFORE, so every group lands between its neighbours whatever order the data arrives in. */
function stackAnchor(group: string): string | undefined {
  const rank = STACK.indexOf(group);
  const groupOf = (layerId: string): string | null => {
    const d = S.catalog?.datasets.find((x) => layerId.startsWith(`ctx-${x.id}-`) || layerId === `dem-${x.id}`);
    return d ? d.group : null;
  };
  for (const l of map.getStyle().layers ?? []) {
    const g = groupOf(l.id);
    if (g !== null && STACK.indexOf(g) > rank) return l.id;
  }
  return map.getLayer("rg-fill") && group === "raster" ? "rg-fill" : map.getLayer("res-fill") ? "res-fill" : undefined;
}

function removeDem(id: string) {
  if (map.getLayer(`dem-${id}`)) map.removeLayer(`dem-${id}`);
  if (map.getSource(`dem-${id}`)) map.removeSource(`dem-${id}`);
}
/** Terrain overlay: read once (range requests, COG overview), then only restyle. */
async function ensureDem(d: CatalogDataset): Promise<void> {
  const state = S.layers[d.id] ?? { visible: false, opacity: 0.65 };
  const cur = S.raster.get(d.id) ?? { loading: false, error: null, overlay: null };
  if (cur.loading) return;
  if (cur.overlay) {
    if (!map.getLayer(`dem-${d.id}`)) addDemLayer(d, cur.overlay, state.opacity);
    else map.setPaintProperty(`dem-${d.id}`, "raster-opacity", state.opacity);
    return;
  }
  S.raster.set(d.id, { loading: true, error: null, overlay: null });
  renderLeft();
  try {
    const overlay = await loadDem(d.raster ?? []);
    S.raster.set(d.id, { loading: false, error: null, overlay });
    if (S.layers[d.id]?.visible) addDemLayer(d, overlay, (S.layers[d.id]?.opacity ?? 0.65));
    log(`terrain: ${overlay.tiles} tiles, ${overlay.tilePx} px each, elevation ${Math.round(overlay.min)}..${Math.round(overlay.max)} m`);
  } catch (e) {
    log(String(e), true);
    S.raster.set(d.id, { loading: false, error: String(e), overlay: null });
  }
  renderLeft();
}
function addDemLayer(d: CatalogDataset, overlay: DemOverlay, opacity: number) {
  removeDem(d.id);
  const [w, s2, e, n] = overlay.bbox;
  map.addSource(`dem-${d.id}`, { type: "image", url: overlay.canvas.toDataURL("image/png"), coordinates: [[w, n], [e, n], [e, s2], [w, s2]] });
  map.addLayer({ id: `dem-${d.id}`, type: "raster", source: `dem-${d.id}`, paint: { "raster-opacity": opacity, "raster-fade-duration": 0 } }, stackAnchor("raster"));
}
async function refreshRasters(): Promise<void> {
  for (const d of S.catalog?.datasets ?? []) {
    if (d.group !== "raster" || !d.raster?.length) continue;
    if (S.layers[d.id]?.visible) await ensureDem(d); else removeDem(d.id);
  }
}
/** Opacity changes restyle the layers that are already on the map; they never ask the server again. */
function restyleContext(d: CatalogDataset) {
  const source = `ctx-${d.id}`;
  if (!map.getSource(source)) return;
  for (const l of map.getStyle().layers ?? []) if (l.id.startsWith(`ctx-${d.id}-`)) map.removeLayer(l.id);
  for (const spec of layerSpecs(d, source, S.layers[d.id]?.opacity ?? 1)) map.addLayer(spec, stackAnchor(d.group));
}

/** The gateway runs 2 queries at a time and answers "busy" (429) beyond that: wait a moment and ask again. */
async function apiRetryBusy<T>(path: string, body?: unknown): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    try {
      return await api<T>(path, body);
    } catch (e) {
      if (attempt >= 8 || !String(e).includes("busy")) throw e;
      await new Promise((done) => setTimeout(done, 250 * (attempt + 1)));
    }
  }
}
/** One refresh at a time; a move during a refresh asks for one more pass (the old pass would show a stale view). */
let contextRefreshing = false;
let contextRefreshAgain = false;
async function refreshContext(): Promise<void> {
  if (contextRefreshing) { contextRefreshAgain = true; return; }
  contextRefreshing = true;
  try {
    do { contextRefreshAgain = false; await refreshContextOnce(); } while (contextRefreshAgain);
  } finally { contextRefreshing = false; }
}
async function refreshContextOnce(): Promise<void> {
  if (!S.catalog) return;
  await refreshRasters();
  const candidates = S.catalog.datasets.filter((d) => sourceIdOf(d) && sourceIdOf(d) !== S.cur?.id && (S.layers[d.id]?.visible ?? false));
  for (const d of candidates) {
    const policy = renderPolicy(d.group, map.getZoom());
    const old = S.context.get(d.id) ?? { features: 0, minZoom: policy.minZoom, limitHit: false, visible: true, loading: false, seq: 0 };
    if (policy.hint) { removeContext(d.id); S.context.set(d.id, { ...old, minZoom: policy.minZoom, loading: false }); continue; }
    const seq = old.seq + 1; S.context.set(d.id, { ...old, loading: true, seq, minZoom: policy.minZoom }); renderLeft();
    try {
      const b = map.getBounds();
      const filter = d.group === "transport" ? classFilter(map.getZoom()) : null;
      const req = { source: sourceIdOf(d), kind: "features", bbox: [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()], crs: d.crs, limit: policy.limit, filter: filter ? { column: "class", op: "in", values: filter } : undefined, browser: false, client: { warm: wasm.isWarm() } };
      const response = await placement.placeWithFallback(await apiRetryBusy<RunResp>("/api/query", req), () => apiRetryBusy<RunResp>("/api/query", { ...req, engine: "duckdb" }));
      const current = S.context.get(d.id);
      if (!current || current.seq !== seq) continue;
      const fc = response.geojson ?? empty;
      fc.features.forEach((f, i) => { (f.properties ??= {}).__i = i; (f.properties ??= {}).__layer = d.id; });
      S.contextData.set(d.id, fc);
      removeContext(d.id);
      map.addSource(`ctx-${d.id}`, { type: "geojson", data: fc });
      for (const spec of layerSpecs(d, `ctx-${d.id}`, S.layers[d.id]?.opacity ?? 1)) map.addLayer(spec, stackAnchor(d.group));
      S.context.set(d.id, { ...current, features: fc.features.length, limitHit: fc.features.length >= policy.limit, loading: false });
    } catch (e) { log(String(e), true); S.context.set(d.id, { ...old, loading: false, seq }); }
    renderLeft();
  }
  const visible = candidates.map((d) => d.id);
  const text = attributionText(S.catalog, visible);
  attributionEl.textContent = visible.length ? `${t("catalog.data_credit")}: ${text}` : "";
  attributionEl.title = text; attributionEl.setAttribute("aria-label", text);
}
// ---------- tabs ----------
function tabList(): [string, Key][] {
  return [["query", "tab.query"], ["results", "tab.results"], ["execution", "tab.execution"], ...(DEV() ? [["profile", "tab.profile"] as [string, Key]] : []), ["logs", "tab.logs"]];
}
function setTab(id: string) { S.tab = id; renderTab(); }
function renderTab() {
  bottom.setAttribute("aria-label", t("a11y.bottom"));
  resizer.setAttribute("aria-label", t("a11y.resize"));
  const ids = tabList().map((x) => x[0]);
  tabsEl.replaceChildren(...tabList().map(([id, key]) =>
    el("button", { type: "button", role: "tab", class: `tab${S.tab === id ? " on" : ""}`, "aria-selected": S.tab === id, "data-testid": `tab-${id}`, tabindex: S.tab === id ? "0" : "-1",
      onclick: () => setTab(id),
      onkeydown: (e: KeyboardEvent) => {
        const i = ids.indexOf(id);
        const go = (j: number) => { e.preventDefault(); setTab(ids[(j + ids.length) % ids.length]); (tabsEl.querySelector('[aria-selected="true"]') as HTMLElement | null)?.focus(); };
        if (e.key === "ArrowRight") go(i + 1);
        if (e.key === "ArrowLeft") go(i - 1);
      } }, t(key))));
  tabBody.replaceChildren();
  if (S.tab === "query") tabBody.append(queryTab());
  else if (S.tab === "results") tabBody.append(resultsTab());
  else if (S.tab === "execution") tabBody.append(executionTab());
  else if (S.tab === "profile" && DEV()) tabBody.append(profileTab());
  else tabBody.append(el("div", { class: "log" }, ...S.logs.map((l) => el("div", { class: l.includes(" ERROR ") ? "err" : "" }, l))));
}

// ---------- demo workflows ----------
// Each demo shows a few datasets, looks at a meaningful place and runs gateway query templates through the same path as every
// other query (/api/query -> PlanGraph -> planner -> executor). The frontend only asks and shows.
function viewportBBox(): number[] {
  const b = map.getBounds();
  return [Math.max(-180, b.getWest()), Math.max(-90, b.getSouth()), Math.min(180, b.getEast()), Math.min(90, b.getNorth())];
}
function moveMap(view: DemoSpec["view"]): Promise<void> {
  return new Promise<void>((done) => {
    const timer = window.setTimeout(done, 1500); // moveend does not fire when the view does not change
    map.once("moveend", () => { window.clearTimeout(timer); done(); });
    map.jumpTo({ center: view.center, zoom: view.zoom });
  });
}
async function queryPlaced(req: Record<string, unknown>): Promise<RunResp> {
  const full = { crs: "OGC:CRS84", bbox: viewportBBox(), browser: true, client: { warm: wasm.isWarm() }, ...req };
  return placement.placeWithFallback(
    await apiRetryBusy<RunResp>("/api/query", full),
    () => apiRetryBusy<RunResp>("/api/query", { ...full, engine: "duckdb", browser: false }),
  );
}
async function runDemo(spec: DemoSpec): Promise<void> {
  if (S.busy || !S.catalog) return;
  S.busy = true; setRunDisabled(true); setStatus("status.running");
  S.demo = { id: spec.id, summary: t("demo.running") };
  renderTab(); renderDemoCard();
  try {
    // show exactly the datasets of this demo
    for (const d of S.catalog.datasets) {
      const isRaster = d.group === "raster" && !!d.raster?.length;
      if (!sourceIdOf(d) && !isRaster) continue;
      S.layers[d.id] = { opacity: S.layers[d.id]?.opacity ?? (isRaster ? 0.65 : 1), visible: spec.layers.includes(d.id) };
    }
    saveLayerStates(S.layers);
    await moveMap(spec.view);
    const bbox = viewportBBox();
    const requests = demoRequests(spec.id).map((r) => ({ ...r, bbox }));
    const activate = (id: string) => { const found = S.sources.find((x) => x.id === id); if (found) S.cur = found; };
    let summary = "";
    if (spec.id === "a") {
      activate(BUILDINGS);
      const r = await queryPlaced(requests[0]);
      await handle(r, t(spec.title));
      summary = t("demo.a.sum", { n: fmtInt(r.rows_total), limit: r.rows_total >= 5000 ? t("demo.limited") : "" });
    } else if (spec.id === "b") {
      activate(BUILDINGS);
      const r = await queryPlaced(requests[0]);
      await handle(r, t(spec.title));
      const c = await queryPlaced(requests[1]);
      summary = t("demo.b.sum", { n: fmtInt(Number(c.rows[0]?.count ?? 0)), d: NEAR_DISTANCE_M });
    } else if (spec.id === "c") {
      activate(BUILDINGS);
      const r = await queryPlaced(requests[0]);
      await handle(r, t(spec.title));
      const top = topGroup(r.rows as { count?: unknown }[], "r_name");
      summary = top ? t("demo.c.sum", { name: top.name, n: fmtInt(top.count) }) : t("demo.empty");
      setTab("results");
    } else if (spec.id === "d") {
      activate(PLACES);
      const r = await queryPlaced(requests[0]);
      await handle(r, t(spec.title));
      const counts = await queryPlaced(requests[1]);
      const { schools, hospitals } = splitPlaces(counts.rows as { category?: unknown; count?: unknown }[]);
      summary = t("demo.d.sum", { n: fmtInt(schools + hospitals), schools: fmtInt(schools), hospitals: fmtInt(hospitals) });
    }
    await refreshContext();
    if (spec.id === "e") {
      const overlay = S.raster.get(DEM)?.overlay;
      summary = overlay ? t("demo.e.sum", { min: fmtInt(Math.round(overlay.min)), max: fmtInt(Math.round(overlay.max)) }) : t("catalog.raster_error");
      setStatus("status.ready");
    }
    S.demo = { id: spec.id, summary };
    renderDemoCard();
  } catch (e) {
    log(String(e), true);
    S.demo = { id: spec.id, summary: t("demo.failed") };
    renderDemoCard();
    setStatus("status.error");
  } finally {
    S.busy = false; setRunDisabled(false);
    renderLeft(); renderTab();
  }
}
/** Result card of the last sample workflow, on the map so it stays visible when the bottom tab changes. */
function renderDemoCard() {
  const spec = DEMOS.find((d) => d.id === S.demo.id);
  demoCardEl.hidden = !spec || !S.demo.summary;
  if (!spec || !S.demo.summary) return;
  fill(demoCardEl,
    el("strong", {}, t(spec.title)),
    el("span", {}, S.demo.summary),
    el("button", { type: "button", "aria-label": t("demo.close"), title: t("demo.close"), onclick: () => { S.demo = { id: null, summary: "" }; renderDemoCard(); } }, "×"));
}

function demoPanel(): HTMLElement | null {
  if (!S.catalog || !S.catalog.datasets.some((d) => d.id === BUILDINGS && d.present)) return null;
  return el("section", { class: "demos", "data-testid": "demos", "aria-label": t("demo.title") },
    el("h3", {}, t("demo.title")),
    ...DEMOS.map((d) => el("div", { class: `demo${S.demo.id === d.id ? " on" : ""}`, "data-testid": `demo-${d.id}` },
      el("div", { class: "demo-text" }, el("strong", {}, t(d.title)), el("span", { class: "meta" }, t(d.desc))),
      el("button", { type: "button", "data-testid": `demo-run-${d.id}`, disabled: S.busy, onclick: () => void runDemo(d) }, t("demo.run")))),
    null);
}

function queryTab(): HTMLElement {
  const cols = S.cur?.columns.filter((c) => c.type !== "other" && typeof c.type === "string") ?? [];
  const lab = (text: string, ctl: HTMLElement) => el("label", {}, el("span", {}, text), ctl);
  const sel = (opts: [string, string][], val: string, on: (v: string) => void, extra: Record<string, unknown> = {}) =>
    el("select", { ...extra, onchange: (e: Event) => on((e.target as HTMLSelectElement).value) }, ...opts.map(([v, l]) => el("option", { value: v, selected: v === val }, l)));
  const colOpts: [string, string][] = [["", t("query.none")], ...cols.map((c): [string, string] => [c.name, DEV() ? c.name : fieldLabel(c.name)])];
  const form = el("div", { class: "form" },
    lab(t("query.kind"), sel([["features", t("query.kind.features")], ["intersects_features", t("query.kind.intersects_features")], ["count_by", t("query.kind.count_by")]], S.kind, (v) => { S.kind = v; renderTab(); }, { "data-testid": "q-kind" })),
    lab(t("query.filter_column"), sel(colOpts, S.filter.column, (v) => { S.filter.column = v; }, { "data-testid": "q-filter-col" })),
    lab(t("query.filter_op"), sel((["eq", "ne", "gt", "ge", "lt", "le"] as const).map((o): [string, string] => [o, t(`op.${o}` as Key)]), S.filter.op, (v) => { S.filter.op = v; })),
    lab(t("query.filter_value"), el("input", { value: S.filter.value, oninput: (e: Event) => { S.filter.value = (e.target as HTMLInputElement).value; } })),
    S.kind === "count_by" ? lab(t("query.group_by"), sel(colOpts.slice(1), S.groupBy, (v) => { S.groupBy = v; })) : null,
    lab(t("query.limit"), el("input", { type: "number", min: 1, max: 100000, value: S.limit, onchange: (e: Event) => { S.limit = Math.min(100000, Math.max(1, Number((e.target as HTMLInputElement).value) || 20000)); } })),
    lab(t("query.where"), sel([["auto", t("query.where.auto")], ["duckdb", t("query.where.server")], ["duckdb-wasm", t("query.where.browser")]], S.engine, (v) => { S.engine = v; if (DEV()) renderHeader(); }, { "data-testid": "q-where" })),
    el("label", { class: "check" }, el("input", { type: "checkbox", checked: S.allowBrowser, onchange: (e: Event) => { S.allowBrowser = (e.target as HTMLInputElement).checked; } }), t("query.allow_browser")),
    el("button", { type: "button", class: "primary", "data-testid": "q-run", onclick: () => void run() }, t("query.run")),
  );
  const out: Child[] = [demoPanel(), form, el("div", { class: "note" }, t("query.hint"))];
  if (DEV()) {
    const ta = el("textarea", { class: "ir", spellcheck: false, "aria-label": t("query.ir"), "data-testid": "ir" }) as HTMLTextAreaElement;
    ta.value = S.last ? JSON.stringify(S.last.ir, null, 2) : "";
    out.push(ta, el("div", { class: "form", style: "margin-top:8px" },
      el("button", { type: "button", onclick: () => void runIr(ta.value, false) }, t("query.run_ir")),
      el("button", { type: "button", onclick: () => void runIr(ta.value, true) }, t("query.explain_ir")),
      el("span", { class: "note" }, t("query.ir_note"))));
  }
  return el("div", {}, out);
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
      await handle(await placement.placeWithFallback(await api<RunResp>("/api/ir/run", body), () => api<RunResp>("/api/ir/run", { ir, engine: "duckdb", browser: false })), "IR run");
    }
  } catch (e) { log(String(e), true); setTab("logs"); }
}

function resultsTab(): HTMLElement {
  const r = S.last;
  if (!r) return el("div", { class: "empty" }, t("results.none"));
  const cols = r.columns.filter((c) => !c.geometry && !c.type.startsWith("Struct"));
  const thead = el("thead", {}, el("tr", {}, ...cols.map((c) => el("th", { scope: "col" }, DEV() ? c.name : fieldLabel(c.name)))));
  const tb = el("tbody", {});
  for (const rw of r.rows) tb.append(el("tr", {}, ...cols.map((c) => el("td", {}, String(rw[c.name] ?? "∅")))));
  const msg = r.rows.length < r.rows_total ? t("results.summary_partial", { n: fmtInt(r.rows_total), shown: fmtInt(r.rows.length) }) : t("results.summary", { n: fmtInt(r.rows_total) });
  return el("div", {}, el("div", { class: "note", style: "margin:0 0 6px" }, msg), el("table", { "data-testid": "results-table" }, thead, tb));
}

// ---------- execution ----------
const ENGINE_LABEL: Record<string, string> = { "duckdb-wasm": "Local Browser / DuckDB-Wasm", duckdb: "Server / DuckDB" };

function executionTab(): HTMLElement {
  const r = S.last;
  if (!r) return el("div", { class: "empty" }, t("exec.none"));
  const sm = summarize(r, S.lastChosen);
  const whereTxt = (w: "server" | "browser") => t(w === "server" ? "exec.where.server" : "exec.where.browser");
  const stat = (label: string, value: string, id: string) => el("div", { class: "stat", "data-testid": id }, el("div", { class: "v" }, value), el("div", { class: "l" }, label));
  const stats = [
    stat(t("exec.time"), fmtMs(sm.timeMs), "stat-time"),
    stat(t("exec.results"), fmtInt(sm.rows), "stat-rows"),
    sm.processBytes != null ? stat(t("exec.data"), `${sm.where === "browser" ? "~" : "≤ "}${fmtBytes(sm.processBytes)}`, "stat-data") : null,
    sm.memoryBytes != null ? stat(t("exec.memory"), `~${fmtBytes(sm.memoryBytes)}`, "stat-memory") : null,
  ];
  const card = el("div", { class: "rec", "data-testid": "exec-summary" },
    el("div", { class: "rec-head" }, icon("ok"), el("span", {}, t("exec.recommended"))),
    el("div", { class: "rec-where", "data-testid": "exec-where", "data-where": sm.where }, icon(sm.where), el("b", {}, whereTxt(sm.where))),
    el("ul", { class: "why" }, ...sm.reasons.map((k) => el("li", {}, icon("ok", 14), el("span", {}, t(k))))),
    el("div", { class: "stats" }, stats),
    sm.alt ? el("div", { class: "alt", "data-testid": "exec-alt" }, icon("info", 14), el("span", {}, `${t("exec.alt")}: ${whereTxt(sm.alt.where)} — ${sm.alt.reason ? t("exec.alt.unavailable", { reason: t(sm.alt.reason) }) : t("exec.alt.slower", { time: sm.alt.timeMs != null ? fmtMs(sm.alt.timeMs) : "—" })}`)) : null);
  const opt = el("div", { class: "opt", "data-testid": "exec-optimize" },
    el("h3", {}, t("exec.optimize")),
    sm.skippedFraction != null
      ? el("ul", {}, el("li", {}, icon("ok", 14), el("span", {}, t("exec.optimize.read_needed"))),
          el("li", {}, icon("ok", 14), el("span", {}, t("exec.optimize.skipped", { pct: fmtPct(sm.skippedFraction) }))),
          sm.processBytes != null ? el("li", {}, icon("ok", 14), el("span", {}, t("exec.optimize.process", { size: fmtBytes(sm.processBytes) }))) : null)
      : el("ul", {}, el("li", {}, icon("info", 14), el("span", {}, t("exec.optimize.none")))));
  const advanced = el("div", { class: "adv", id: "adv", "data-testid": "advanced", hidden: !DEV() && !S.advancedOpen },
    plannerPanel(r), metricCards(r.metrics, r.timing), engineSteps(r.physical, r),
    el("h3", {}, t("dev.planner_decisions")),
    ...r.physical.decisions.map((d) => el("div", { class: "dec" }, el("span", { class: `o-${d.outcome}` }, `[${d.outcome}] `), `${d.rule} · ${d.subject}: ${d.reason}`)),
    ...r.metrics.notes.map((n) => el("div", { class: "note" }, n)));
  const toggle = DEV() ? null : el("button", { type: "button", class: `adv-toggle${S.advancedOpen ? " open" : ""}`, "data-testid": "advanced-toggle", "aria-expanded": S.advancedOpen, "aria-controls": "adv", onclick: () => { S.advancedOpen = !S.advancedOpen; renderTab(); } },
    icon("chevron", 14), el("span", {}, t("exec.advanced")));
  return el("div", { class: "exec" }, card, opt, toggle, advanced);
}

function engineSteps(p: Physical, r: RunResp): HTMLElement {
  const pipe = el("div", { class: "pipe" });
  const steps = [...p.steps.map((s) => ({ t: s.label, d: s.detail })), { t: "Render", d: `${r.rows_total} rows → table / map (GeoJSON boundary conversion ${fmtMs(r.timing.boundary_conversion_ms.value ?? 0)})` }];
  steps.forEach((s, i) => {
    if (i) pipe.append(el("div", { class: "arrow", "aria-hidden": "true" }, "↓"));
    pipe.append(el("div", { class: "step" }, el("div", { class: "t" }, s.t), el("div", { class: "d" }, s.d.replace("\\\\?\\", ""))));
  });
  return pipe;
}

// Developer: metrics with provenance badges
const BADGE: Record<string, [Key, string]> = {
  engine_observed: ["metric.badge.observed", "ok"], source_observed: ["metric.badge.observed", "ok"],
  source_estimated: ["metric.badge.estimated", "est"], planner_estimated: ["metric.badge.estimated", "est"],
  derived: ["metric.badge.derived", "der"], unknown: ["metric.badge.unavailable", "na"],
};
function mcard(label: string, m: Metric<number>, fmt: (v: number) => string, prefix = ""): HTMLElement {
  const [text, cls] = m.value == null ? BADGE.unknown : (BADGE[m.source] ?? BADGE.unknown);
  return el("div", { class: "card", "data-metric": label, "data-source": m.value == null ? "unknown" : m.source },
    el("div", { class: "v" }, m.value == null ? t("metric.unknown") : prefix + fmt(m.value)),
    el("div", { class: "l" }, label, " ", el("span", { class: `badge ${cls}` }, t(text))));
}
function metricCards(m: Metrics, tm: RunResp["timing"]): HTMLElement {
  const skipped = m.row_groups_skipped.value, total = m.row_groups_total.value;
  const rg: Metric<number> = { value: skipped, source: m.row_groups_skipped.source };
  return el("div", { class: "cards" },
    mcard(t("metric.total"), tm.total_ms, fmtMs), mcard(t("metric.engine"), m.elapsed_ms, fmtMs), mcard(t("metric.rows"), m.rows_returned, fmtInt),
    mcard(t("metric.rg_total"), m.row_groups_total, fmtInt), mcard(t("metric.rg_skipped", { total: total ?? "?" }), rg, fmtInt),
    mcard(t("metric.rows_scanned"), m.rows_scanned_upper_bound, fmtInt, "≤ "), mcard(t("metric.bytes_ub"), m.estimated_bytes_upper_bound, fmtBytes, "≤ "),
    mcard(t("metric.bytes_read"), m.bytes_read, fmtBytes), mcard(t("metric.peak"), m.peak_memory_bytes, fmtBytes));
}

/** Developer: which engine runs the plan, the alternatives, and the planner's assessment of its own decision. */
function plannerPanel(r: RunResp): HTMLElement {
  const p = r.physical;
  const sel = p.candidates?.find((c) => c.engine === p.engine);
  const alts = (p.candidates ?? []).filter((c) => c.engine !== p.engine);
  const where = p.placement === "browser" ? "Local (browser)" : "Server";
  const inputMb = p.input_bytes_upper_bound == null ? "—" : `≤ ${fmtBytes(p.input_bytes_upper_bound)}`;
  const transfer = sel?.cost ? fmtBytes(sel.cost.transfer_bytes) : "—";
  return el("div", { class: "planner", "data-testid": "planner-panel" },
    el("div", { class: "row" },
      el("span", { class: `engine ${p.placement ?? "server"}`, "data-testid": "engine-selected", "data-engine": p.engine }, ENGINE_LABEL[p.engine] ?? p.engine),
      el("span", { class: "badge der" }, where),
      sel?.score_ms != null ? el("span", { class: "note" }, `${t("dev.score")} ${sel.score_ms.toFixed(1)} ms (${t("dev.lower_wins")})`) : null),
    el("div", { class: "kv2" }, el("span", {}, t("dev.est_input")), el("b", {}, inputMb),
      el("span", {}, p.placement === "browser" ? t("dev.est_transfer_browser") : t("dev.est_transfer_server")), el("b", {}, `~${transfer}`)),
    ...(sel?.reasons ?? []).map((x) => el("div", { class: "dec" }, "✓ ", x)),
    ...alts.map((c: Candidate) => el("div", { class: "alt dev", "data-testid": "engine-alternative", "data-engine": c.engine },
      el("b", {}, `${t("dev.alternative")}: ${ENGINE_LABEL[c.engine] ?? c.engine}`), "  ",
      c.feasible ? `${t("dev.score")} ${c.score_ms?.toFixed(1)} ms` : t("dev.not_feasible"), c.feasible ? "" : ` — ${c.rejected_because.join("; ")}`)),
    r.fallback ? el("div", { class: "dec", "data-testid": "engine-fallback" }, el("span", { class: "o-warning" }, "⚠ "), t("dev.fallback_occurred", { cls: r.fallback.class ?? "?", reason: r.fallback.reason.slice(0, 160) })) : null,
    r.result_digest ? el("div", { class: "note" }, t("dev.digest", { hash: r.result_digest.sha256.slice(0, 12), count: r.result_digest.count })) : null,
    assessmentPanel(r));
}

const pct = (v: number) => `${Math.round(v * 100)}%`;
function badge(text: string, cls: string, testid?: string): HTMLElement {
  const mark = cls === "ok" ? "✓ " : cls === "warn" ? "⚠ " : cls === "bad" ? "✕ " : "";
  return el("span", { class: `badge ${cls}`, "data-testid": testid }, mark + text);
}
const HEALTH_BADGE: Record<Health["state"], [Key, string]> = { healthy: ["dev.health.healthy", "ok"], degraded: ["dev.health.degraded", "warn"], unhealthy: ["dev.health.unhealthy", "bad"], unknown: ["dev.health.unknown", "na"] };

function assessmentPanel(r: RunResp): HTMLElement {
  const p = r.physical, a: Assessment | undefined = p.assessment;
  if (!a) return el("div", {});
  const browser = p.candidates?.find((c) => c.kind === "browser");
  const probe = a.probes?.["duckdb-wasm"];
  const kv = (k: string, ...v: Child[]) => [el("span", {}, k), el("b", {}, v)] as HTMLElement[];
  const conf = a.confidence;
  const confCls = conf.label === "high" ? "ok" : conf.label === "medium" ? "warn" : "bad";
  const confText = t(conf.label === "low" ? "dev.low_confidence" : conf.label === "medium" ? "dev.medium_confidence" : "dev.high_confidence");
  const est = a.estimate;
  const factor = (e: number, tr: number) => { const x = (e + 1) / (tr + 1); return Math.max(x, 1 / x); };
  const risk = browser?.risk;
  const dev = placement.state.probe?.device;
  const rows: HTMLElement[] = [
    ...kv(t("dev.engine"), ENGINE_LABEL[p.engine] ?? p.engine, " ", badge(p.placement === "browser" ? "Local" : "Server", "der")),
    ...kv(t("dev.confidence"), badge(`${confText} ${pct(conf.score)}`, confCls, "confidence-badge"),
      el("span", { class: "note", "data-testid": "confidence-factors", title: conf.factors.map((f) => `${f.name} ${f.value.toFixed(2)} (w ${f.weight})`).join("\n") }, `  engine ${pct(a.engine_confidence)} × margin ${a.margin_factor.toFixed(2)}`)),
    ...kv(t("dev.health"), Object.entries(a.health ?? {}).flatMap(([e, h]) => [badge(`${e}: ${t(HEALTH_BADGE[h.state][0])}`, HEALTH_BADGE[h.state][1], `health-${e}`), " "])),
    ...kv(t("dev.probe"), probe ? [badge(probe.status === "fresh" && probe.usable ? t("dev.observed") : probe.status === "missing" ? t("dev.not_probed") : probe.status === "stale" ? t("dev.stale") : t("dev.failed"), probe.status === "fresh" && probe.usable ? "ok" : "warn", "probe-badge"),
      el("span", { class: "note" }, probe.age_ms != null ? `  ${(probe.age_ms / 1000).toFixed(0)} s · ${probe.memory_class ?? "?"} · wasm ${probe.engine_version ?? "?"}${dev ? ` · ${dev.browser_family} ${dev.browser_major ?? ""}` : ""}${probe.failed_checks.length ? ` · not OK: ${probe.failed_checks.join(", ")}` : ""}` : "")] : "—"),
    ...kv(t("dev.network_profile"), `${a.network.name} `, el("span", { class: "note" }, `(rtt ${a.network.rtt_ms} ms · ${a.network.down_mbps} Mbps · ${a.network.origin})`)),
    ...kv(t("dev.result_estimate"), badge(t("dev.estimated"), "est"), ` ~${fmtInt(est.rows)} (${fmtInt(est.low)}–${fmtInt(est.high)}), ${est.quality}${est.bounded_by_limit ? (est.limit_certain ? " · LIMIT" : " · capped by LIMIT") : ""}`,
      r.rows_total != null ? [" · ", badge(t("dev.observed"), "ok"), el("span", { "data-testid": "estimate-vs-actual" }, ` ${fmtInt(r.rows_total)} (×${factor(est.rows, r.rows_total).toFixed(1)})`)] : []),
    ...kv(t("dev.browser_risk"), risk
      ? [badge(t(`dev.safety.${risk.safety}` as Key), risk.safety === "safe" ? "ok" : risk.safety === "risky" ? "warn" : risk.safety === "unsafe" ? "bad" : "na", "safety-badge"),
          el("span", { class: "note", "data-testid": "risk" }, ` ${pct(risk.score)} · ≤ ${fmtBytes(risk.fetch_upper_bytes)} / ${fmtBytes(risk.envelope.unsafe_fetch_bytes)}`)]
      : browser && !browser.feasible ? `✕ ${browser.rejected_because[0]?.slice(0, 120) ?? ""}` : "—"),
    ...kv(t("dev.fallback"), a.fallback ? t("dev.fallback_text", { engine: ENGINE_LABEL[a.fallback.engine] ?? a.fallback.engine, n: a.fallback.max_fallbacks }) : p.placement === "browser" ? t("dev.fallback_none") : t("dev.fallback_none_server")),
  ];
  return el("div", { class: "kv2 assess", "data-testid": "assessment" }, rows,
    ...(a.policy ?? []).map((x) => el("div", { class: "dec", style: "grid-column:1/-1" }, el("span", { class: "o-warning" }, `${t("dev.policy")} `), x)));
}

function profileTab(): HTMLElement {
  const r = S.last;
  if (!r) return el("div", { class: "empty" }, t("profile.none"));
  return el("div", {}, metricCards(r.metrics, r.timing),
    el("table", {}, el("tbody", {},
      el("tr", {}, el("th", { scope: "row" }, t("profile.gateway")), el("td", {}, fmtMs(r.timing.plan_and_overhead_ms.value ?? 0))),
      el("tr", {}, el("th", { scope: "row" }, t("profile.engine")), el("td", {}, fmtMs(r.metrics.elapsed_ms.value ?? 0))),
      el("tr", {}, el("th", { scope: "row" }, t("profile.boundary")), el("td", {}, fmtMs(r.timing.boundary_conversion_ms.value ?? 0))))),
    el("div", { class: "note" }, t("profile.note")));
}

// ---------- bottom pipeline strip ----------
const ICONED = new Set(["source", "bbox_filter", "intersects", "filter", "project", "aggregate", "sort", "limit", "buffer", "spatial_join", "server", "browser"]);
function renderPipeline() {
  const r = S.last;
  pipeEl.setAttribute("aria-label", t("exec.steps"));
  if (!r) { pipeEl.replaceChildren(el("span", { class: "empty" }, t("pipe.none"))); return; }
  const steps = pipelineSteps(r.ir as PlanGraph, r.physical.placement === "browser" ? "browser" : "server");
  const nodes: Child[] = [];
  steps.forEach((s, i) => {
    if (i) nodes.push(el("span", { class: "link", "aria-hidden": "true" }, "→"));
    nodes.push(el("div", { class: `node${i === steps.length - 1 ? " last" : ""}`, "data-testid": "pipe-node", "data-step": s.key },
      el("span", { class: "ic" }, icon(ICONED.has(s.key) ? s.key : "filter", 18)),
      el("span", { class: "tx" }, el("span", { class: "t" }, t(s.title)), el("span", { class: "s" }, t(s.sub)))));
  });
  pipeEl.replaceChildren(...nodes.map((n) => n as Node));
}

// ---------- everything that depends on language or mode ----------
function renderAll() {
  document.body.dataset.mode = S.mode;
  mapDiv.setAttribute("aria-label", t("a11y.map"));
  renderHeader(); renderLegend(); renderLeft(); renderRight(); renderTab(); renderPipeline(); renderPopover(); applyMapVisibility(); renderDemoCard();
}

applyBottomH();
document.documentElement.lang = getLang();
app.classList.add("no-inspector");
renderAll();

// Test hook for the browser tests only (?e2e=1): exposes the map and state, nothing else.
if (params.has("e2e")) {
  const gf = {
    map, state: S, select, setTab, setMode, setLang: changeLang, wasm, resolvePlacement: placement.resolvePlacement, bindSources: placement.bindSources,
    probeRuntime: placement.probeRuntime, placeWithFallback: placement.placeWithFallback, reportRun: placement.reportRun, compile, placement, run,
    focusOnResult, applyMapVisibility,
    /** What the result layer really holds: feature count, distinct geometries, and the largest feature relative to the dataset extent. */
    async resultInfo() {
      const src = map.getSource("result") as maplibregl.GeoJSONSource;
      const fc = (await src.getData()) as GeoJSON.FeatureCollection;
      const keys = new Set(fc.features.map((f) => JSON.stringify(f.geometry)));
      const ext = extentOf(fc);
      const ds = S.cur?.bbox ?? null;
      const dsArea = ds ? (ds[2] - ds[0]) * (ds[3] - ds[1]) : 0;
      const maxFeatArea = Math.max(0, ...fc.features.map((f) => { const b = extentOf({ type: "FeatureCollection", features: [f] }); return b ? (b[2] - b[0]) * (b[3] - b[1]) : 0; }));
      return { features: fc.features.length, distinct: keys.size, extent: ext, maxFeatureShareOfDataset: dsArea ? maxFeatArea / dsArea : 0 };
    },
    basemapLoaded: () => basemapLoaded, baseLayerIds: () => baseLayerIds,
    layerVisibility: (id: string) => (map.getLayer(id) ? map.getLayoutProperty(id, "visibility") ?? "visible" : null),
    layerOrder: () => map.getStyle().layers.map((l) => l.id),
    layers: S.layers,
    contextInfo: () => Object.fromEntries([...S.context.entries()].map(([id, x]) => [id, { id, features: x.features, minZoom: x.minZoom, limitHit: x.limitHit, visible: x.visible }])),
  };
  // tests read `state.probe`; the probe state lives in the placement module
  Object.defineProperty(S, "probe", { get: () => placement.state.probe, set: (v) => { placement.state.probe = v; }, enumerable: true });
  (window as unknown as Record<string, unknown>).__gf = gf;
}
