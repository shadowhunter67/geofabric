import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { compile, initLowering } from "../src/engine/lowering.ts";
import { planRequest, prune, StaticError } from "../src/static/adapter.ts";
import type { QueryReq, StaticSource } from "../src/static/adapter.ts";
import { handle, newState, NOT_IN_DEPLOYMENT } from "../src/static/gateway.ts";
import type { GatewayDeps } from "../src/static/gateway.ts";

await initLowering(readFileSync(new URL("../src/engine/generated/geo_lowering.wasm", import.meta.url)));

// the Source nodes of the golden request fixtures (the same ones the Rust tests build plans from)
const fixtureSources = JSON.parse(readFileSync(new URL("../../crates/geo-plan/fixtures/requests/_sources.json", import.meta.url), "utf8")) as Record<string, { uri: string }>;
const node = (id: string) => ({ ...fixtureSources[id], uri: `demo/v1/${id}.parquet.zip` });

const places: StaticSource = {
  ir_source: node("places"),
  id: "places", size_bytes: 9_000_000, rows: 300, row_groups: 3, crs: "OGC:CRS84", geometry_column: "geometry", geometry_type: "point",
  bbox: [106.6, 10.7, 106.8, 10.9], file: "demo_places.parquet",
  columns: [{ name: "id", type: "utf8" }, { name: "category", type: "utf8" }, { name: "geometry", type: { other: "geometry" } }, { name: "bbox", type: { other: "struct" } }],
  row_group_stats: [
    { index: 0, rows: 100, bytes: 3_000_000, bbox: [106.6, 10.7, 106.65, 10.75] },
    { index: 1, rows: 100, bytes: 3_000_000, bbox: [106.69, 10.77, 106.72, 10.8] },
    { index: 2, rows: 100, bytes: 3_000_000, bbox: [106.75, 10.85, 106.8, 10.9] },
  ],
};
const bundled = { version: "v1", host: null, files: { places: { file: "demo_places.parquet.zip", path: "v1/demo_places.parquet.zip", bytes: 9_000_000, sha256: "x" } } };
const deps: GatewayDeps = {
  pack: { sources: [places], catalog: { datasets: [], attributions: [], errors: [] }, manifest: bundled },
  fileUrl: (id) => `https://example.test/geofabric/demo/${bundled.files[id as "places"].path}`,
};
// the same pack on a host that was VERIFIED to answer ranged requests (the manifest says so: data/scripts/publish_demo_data.py)
const remoteManifest = { ...bundled, host: { kind: "jsdelivr-github", base_url: "https://cdn.example.test/gh/o/r@abc/data/public-demo/", range: true, commit: "abc" },
  files: { places: { ...bundled.files.places, url: "https://cdn.example.test/gh/o/r@abc/data/public-demo/v1/demo_places.parquet.zip" } } };
const remoteDeps: GatewayDeps = { pack: { ...deps.pack, manifest: remoteManifest }, fileUrl: (id) => remoteManifest.files[id as "places"].url };
const req = (over: Partial<QueryReq> = {}): QueryReq => ({ source: "places", kind: "features", bbox: [106.68, 10.76, 106.73, 10.81], crs: "OGC:CRS84", ...over });

test("the adapter hands the request to the canonical builder and returns its plan, the capability verdict and the dataset's uri", () => {
  const r = planRequest(req({ limit: 500 }), [places]);
  assert.equal(r.plan.ir_version, "0.3");
  assert.deepEqual(Object.values(r.plan.nodes).map((n) => n.op).sort(), ["bbox_filter", "limit", "project", "source"]);
  assert.deepEqual(r.missingOps, []);
  assert.equal(r.uri, "demo/v1/places.parquet.zip");
  // the engine hint is a planner input attached afterwards, not part of the request or the plan semantics
  assert.deepEqual(planRequest(req({ engine: "duckdb-wasm" }), [places]).plan.hints, { preferred_engine: "duckdb-wasm" });
  assert.equal(planRequest(req(), [places]).planJson, planRequest(req({ engine: "duckdb-wasm" }), [places]).planJson, "an engine hint never changes the plan's nodes");
});

test("the built plan compiles with the real browser lowering (the SQL carries the covering-column bbox test)", () => {
  const { plan } = planRequest(req({ filter: { column: "category", op: "eq", value: "cafe" } }), [places]);
  const sql = compile(plan, { "demo/v1/places.parquet.zip": { url: "https://example.test/p.parquet", covering: { xmin: "bbox.xmin", ymin: "bbox.ymin", xmax: "bbox.xmax", ymax: "bbox.ymax", declared: false }, access: "range" } }, 1000);
  assert.match(sql.sql, /"bbox"\."xmin"/);
});

test("refused requests carry the builder's structured code and the legacy error string", () => {
  const refuse = (q: QueryReq) => { try { planRequest(q, [places]); } catch (e) { return e as StaticError; } throw new Error("expected a refusal"); };
  assert.deepEqual([refuse(req({ crs: "EPSG:32648" })).code, refuse(req({ crs: "EPSG:32648" })).planCode], ["crs_mismatch", "INVALID_CRS"]);
  assert.equal(refuse(req({ filter: { column: "no_such", op: "eq", value: "x" } })).planCode, "INVALID_FIELD");
  assert.equal(refuse(req({ source: "ghost" })).planCode, "UNKNOWN_DATASET");
  assert.equal(refuse(req({ kind: "teleport" })).planCode, "UNSUPPORTED_REQUEST");
});

test("what the browser cannot run is a valid plan with a capability verdict, never an approximation and never silently dropped", () => {
  const roads: StaticSource = { ...places, id: "roads", ir_source: node("roads") };
  const near = planRequest(req({ source: "places", kind: "near_count", with: { source: "roads", distance_m: 200 } }), [places, roads]);
  assert.ok(near.missingOps.includes("transform") && near.missingOps.includes("spatial_semi_join"), String(near.missingOps));
  assert.ok(Object.values(near.plan.nodes).some((n) => n.op === "spatial_semi_join"), "the semi join is in the plan, exactly as built");
  assert.deepEqual(planRequest(req({ kind: "intersects_features" }), [places]).missingOps, ["intersects"]);
});

test("row groups are pruned by their boxes; a group without a box is always kept", () => {
  const p = prune(places, [106.68, 10.76, 106.73, 10.81]);
  assert.deepEqual(p.kept, [1]);
  assert.equal(p.skipped, 2);
  assert.equal(p.kept_rows, 100);
  const unboxed = { ...places, row_group_stats: [{ index: 0, rows: 5, bytes: 10, bbox: null }] };
  assert.deepEqual(prune(unboxed, [0, 0, 1, 1]).kept, [0]);
});

test("/api/query answers a browser placement bound to the published file, and reports the absent engines honestly", async () => {
  const st = newState();
  const r = await handle("POST", "/api/query", req(), deps, st);
  assert.equal(r.status, 200);
  const b = r.body as { placement: string; source_urls: Record<string, string>; physical: { engine: string; candidates: { engine: string; feasible: boolean; rejected_because: string[] }[]; scans: { prune: { kept: number[] } }[] } };
  assert.equal(b.placement, "browser");
  assert.equal(b.physical.engine, "duckdb-wasm");
  assert.deepEqual(Object.values(b.source_urls), ["https://example.test/geofabric/demo/v1/demo_places.parquet.zip"]);
  assert.deepEqual(b.physical.scans[0].prune.kept, [1]);
  const byEngine = Object.fromEntries(b.physical.candidates.map((c) => [c.engine, c]));
  assert.equal(byEngine["duckdb-wasm"].feasible, true);
  for (const e of ["duckdb", "postgis"]) {
    assert.equal(byEngine[e].feasible, false);
    assert.deepEqual(byEngine[e].rejected_because, [NOT_IN_DEPLOYMENT]);
  }
});

test("a server engine hint or declining the browser is an explicit 'requires GeoFabric Server', not a fake answer", async () => {
  const st = newState();
  for (const over of [{ engine: "duckdb" }, { engine: "postgis" }, { browser: false }]) {
    const r = await handle("POST", "/api/query", req(over), deps, st);
    assert.equal(r.status, 422);
    assert.equal((r.body as { error: string }).error, "server_required");
  }
  const near = await handle("POST", "/api/query", req({ kind: "near", with: { source: "places", distance_m: 200 } }), deps, st);
  assert.equal(near.status, 422);
  assert.match((near.body as { message: string }).message, /requires GeoFabric Server \(the browser engine cannot run: .*transform.*\)/);
});

test("catalog, sources, row groups and state come from the published pack; PostGIS is 'not configured'", async () => {
  const st = newState();
  const sources = await handle("GET", "/api/sources", undefined, deps, st);
  const list = (sources.body as { sources: { id: string; file?: string }[] }).sources;
  assert.equal(list[0].id, "places");
  assert.equal("file" in list[0], false);
  const rg = await handle("GET", "/api/sources/places/row_groups", undefined, deps, st);
  assert.equal((rg.body as { row_groups: unknown[] }).row_groups.length, 3);
  const state = await handle("GET", "/api/state", undefined, deps, st);
  assert.deepEqual((state.body as { postgis: unknown }).postgis, { configured: false });
  assert.equal((await handle("GET", "/api/sources/nope/row_groups", undefined, deps, st)).status, 404);
  assert.equal((await handle("GET", "/api/not-a-route", undefined, deps, st)).status, 404);
});

test("the browser's reports feed health: a failure degrades, three make it unhealthy", async () => {
  const st = newState();
  const h = async () => ((await handle("GET", "/api/state", undefined, deps, st)).body as { health: Record<string, { state: string }> }).health["duckdb-wasm"].state;
  assert.equal(await h(), "unknown");
  await handle("POST", "/api/report", { ok: true }, deps, st);
  assert.equal(await h(), "healthy");
  await handle("POST", "/api/report", { ok: false, message: "boom" }, deps, st);
  assert.equal(await h(), "degraded");
  await handle("POST", "/api/report", { ok: false, message: "boom" }, deps, st);
  await handle("POST", "/api/report", { ok: false, message: "boom" }, deps, st);
  assert.equal(await h(), "unhealthy");
});

test("on a host verified for ranges the layer is read by range, from the manifest's absolute URL; only the pruned bytes are estimated", async () => {
  const st = newState();
  const r = await handle("POST", "/api/query", req(), remoteDeps, st);
  const b = r.body as { source_urls: Record<string, string>; physical: { steps: { label: string }[]; assessment: { browser_access: { strategy: string; fetch_bytes_est: number; file_bytes: number; requests_est: number }[] } } };
  assert.deepEqual(Object.values(b.source_urls), ["https://cdn.example.test/gh/o/r@abc/data/public-demo/v1/demo_places.parquet.zip"]);
  const acc = b.physical.assessment.browser_access[0];
  assert.equal(acc.strategy, "range");
  assert.ok(acc.fetch_bytes_est < acc.file_bytes / 2, `estimated ${acc.fetch_bytes_est} of ${acc.file_bytes}`);
  assert.ok(b.physical.steps.some((x) => x.label === "Fetch GeoParquet byte ranges"));
});

test("a tiny layer is cheaper whole even on a range host", async () => {
  const tiny = { ...places, size_bytes: 200_000, row_group_stats: [{ index: 0, rows: 5, bytes: 200_000, bbox: [106.6, 10.7, 106.8, 10.9] as [number, number, number, number] }] };
  const d: GatewayDeps = { ...remoteDeps, pack: { ...remoteDeps.pack, sources: [tiny] } };
  const b = (await handle("POST", "/api/query", req(), d, newState())).body as { physical: { assessment: { browser_access: { strategy: string }[] } } };
  assert.equal(b.physical.assessment.browser_access[0].strategy, "full");
});

test("pack files are fetched whole (GitHub Pages cannot serve the ranged HEAD DuckDB-Wasm needs): every strategy is 'full' and the step says so", async () => {
  const st = newState();
  const r = await handle("POST", "/api/query", req(), deps, st);
  const b = r.body as { physical: { steps: { label: string }[]; assessment: { browser_access: { strategy: string; fetch_bytes_est: number; file_bytes: number }[] } } };
  assert.equal(b.physical.assessment.browser_access[0].strategy, "full");
  assert.equal(b.physical.assessment.browser_access[0].fetch_bytes_est, b.physical.assessment.browser_access[0].file_bytes);
  assert.ok(b.physical.steps.some((s) => s.label === "Fetch the GeoParquet file"));
});
