import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { compile, initLowering } from "../src/engine/lowering.ts";
import { buildPlan, prune, StaticError } from "../src/static/ir.ts";
import type { QueryReq, StaticSource } from "../src/static/ir.ts";
import { handle, newState, NOT_IN_DEPLOYMENT } from "../src/static/gateway.ts";
import type { GatewayDeps } from "../src/static/gateway.ts";

await initLowering(readFileSync(new URL("../src/engine/generated/geo_lowering.wasm", import.meta.url)));

const places: StaticSource = {
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

test("a features request becomes the gateway's plan: source, bbox filter, project (no struct columns), limit", () => {
  const { ir } = buildPlan(req({ limit: 500 }), places, "demo/demo_places.parquet");
  assert.equal(ir.ir_version, "0.3");
  const ops = Object.values(ir.nodes).map((n) => n.op).sort();
  assert.deepEqual(ops, ["bbox_filter", "limit", "project", "source"]);
  const proj = Object.values(ir.nodes).find((n) => n.op === "project");
  assert.deepEqual(proj && proj.op === "project" ? proj.columns : null, ["id", "category", "geometry"]);
  const lim = Object.values(ir.nodes).find((n) => n.op === "limit");
  assert.equal(lim && lim.op === "limit" ? lim.n : null, 500);
});

test("the limit is capped at the gateway's maximum", () => {
  const { ir } = buildPlan(req({ limit: 5_000_000 }), places, "u");
  const lim = Object.values(ir.nodes).find((n) => n.op === "limit");
  assert.equal(lim && lim.op === "limit" ? lim.n : null, 100_000);
});

test("count_by sorts by count then by the group key, and keeps the top 50", () => {
  const { ir } = buildPlan(req({ kind: "count_by", group_by: "category" }), places, "u");
  const sort = Object.values(ir.nodes).find((n) => n.op === "sort");
  assert.deepEqual(sort && sort.op === "sort" ? sort.keys : null, [{ column: "count", descending: true }, { column: "category", descending: false }]);
  const lim = Object.values(ir.nodes).find((n) => n.op === "limit");
  assert.equal(lim && lim.op === "limit" ? lim.n : null, 50);
});

test("filters: compare, in, and typed literals", () => {
  const eq = buildPlan(req({ filter: { column: "category", op: "eq", value: "restaurant" } }), places, "u").ir;
  assert.ok(Object.values(eq.nodes).some((n) => n.op === "filter" && n.predicate.kind === "compare" && JSON.stringify(n.predicate.value) === '{"str":"restaurant"}'));
  const inn = buildPlan(req({ filter: { column: "category", op: "in", values: ["a", "b"] } }), places, "u").ir;
  assert.ok(Object.values(inn.nodes).some((n) => n.op === "filter" && n.predicate.kind === "in" && n.predicate.values.length === 2));
  assert.throws(() => buildPlan(req({ filter: { column: "category", op: "in", values: [] } }), places, "u"), StaticError);
  assert.throws(() => buildPlan(req({ filter: { column: "category", op: "like", value: "x" } }), places, "u"), StaticError);
});

test("the plan compiles with the real browser lowering (the SQL carries the covering-column bbox test)", () => {
  const { ir } = buildPlan(req({ filter: { column: "category", op: "eq", value: "cafe" } }), places, "demo/p.parquet");
  const sql = compile(ir, { "demo/p.parquet": { url: "https://example.test/p.parquet", covering: { xmin: "bbox.xmin", ymin: "bbox.ymin", xmax: "bbox.xmax", ymax: "bbox.ymax", declared: false }, access: "range" } }, 1000);
  assert.match(sql.sql ?? String(sql), /bbox\.xmin|"bbox"\."xmin"/);
});

test("what needs the server is refused with its reason, never approximated", () => {
  for (const kind of ["near", "near_count", "join_count_by", "intersects_features"]) {
    assert.throws(() => buildPlan(req({ kind }), places, "u"), (e: unknown) => e instanceof StaticError && e.code === "server_required" && /GeoFabric Server/.test(e.message));
  }
  assert.throws(() => buildPlan(req({ crs: "EPSG:32648" }), places, "u"), (e: unknown) => e instanceof StaticError && e.code === "crs_mismatch");
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
  const near = await handle("POST", "/api/query", req({ kind: "near" }), deps, st);
  assert.equal(near.status, 422);
  assert.match((near.body as { message: string }).message, /requires GeoFabric Server/);
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
