import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// Static browser profile: GeoFabric opened from a (non-root) URL with NO Gateway and NO PostGIS. Runs against the local `vite preview`
// of the production bundle, or against the deployed site (PUBLIC_URL). The map style/tiles come from a public server; they are blocked
// in every test except the one that checks the basemap, so the tests do not depend on it.

type GF = {
  state: { last: { geojson: GeoJSON.FeatureCollection | null; physical: { engine: string; placement?: string }; rows_total: number } | null; probe: { checks: { name: string; ok: boolean; required: boolean }[] } | null; settling: boolean };
  select: (f: GeoJSON.Feature) => void;
  resolvePlacement: (r: unknown) => Promise<{ rows: Record<string, unknown>[]; rows_total: number; geojson: GeoJSON.FeatureCollection | null }>;
  basemapLoaded: () => boolean;
  layers: Record<string, { visible: boolean; opacity: number }>;
  applyMapVisibility: () => Promise<void>;
  contextInfo: () => Record<string, { features: number; loading: boolean }>;
};
declare global { interface Window { __gf: GF } }

const TILES = /tiles\.openfreemap\.org/;

async function open(page: Page, o: { lang?: "vi" | "en"; mode?: "explorer" | "developer"; tiles?: "blocked" | "network" } = {}) {
  if (o.tiles !== "network") await page.route(TILES, (r) => r.abort());
  const requests: string[] = [];
  page.on("request", (r) => requests.push(r.url()));
  await page.goto(`./?e2e=1&mode=${o.mode ?? "explorer"}&lang=${o.lang ?? "vi"}`);
  await expect(page.getByTestId("status")).toContainText(/results|kết quả/, { timeout: 90_000 });
  await page.waitForFunction(() => window.__gf.state.settling === false, undefined, { timeout: 90_000 });
  return requests;
}

test("opens from the base path, shows real building polygons from DuckDB-Wasm, with no gateway", async ({ page }) => {
  const requests = await open(page);
  const last = await page.evaluate(() => ({ engine: window.__gf.state.last!.physical.engine, placement: window.__gf.state.last!.physical.placement, n: window.__gf.state.last!.geojson!.features.length, types: [...new Set(window.__gf.state.last!.geojson!.features.map((f) => f.geometry.type))] }));
  expect(last.engine).toBe("duckdb-wasm");
  expect(last.placement).toBe("browser");
  expect(last.n).toBeGreaterThan(100);
  expect(last.types).toContain("Polygon");
  // the data came from the published pack, by HTTP (range) requests, next to the app
  expect(requests.some((u) => /\/geofabric\/demo\/v1\/demo_buildings\.parquet\.zip$/.test(u))).toBe(true);
});

test("the DuckDB-Wasm runtime probe passes on this origin (wasm + worker + range reads + a real query)", async ({ page }) => {
  await open(page);
  await page.waitForFunction(() => window.__gf.state.probe !== null, undefined, { timeout: 60_000 });
  const probe = await page.evaluate(() => window.__gf.state.probe!.checks.map((c) => ({ name: c.name, ok: c.ok, required: c.required })));
  expect(probe.length).toBeGreaterThan(3);
  expect(probe.filter((c) => c.required && !c.ok), "every required probe check passes").toEqual([]);
});

test("network audit: only the app, its published data and the basemap host are contacted; nothing private", async ({ page }) => {
  const requests = await open(page, { tiles: "network" });
  const origin = new URL(page.url()).origin;
  const hosts = new Set(requests.map((u) => new URL(u).host));
  const external = [...hosts].filter((h) => h !== new URL(origin).host && !/^(data|blob):/.test(h) && h !== "");
  // openfreemap.org = basemap tiles; extensions.duckdb.org = DuckDB-Wasm loads its `parquet` extension (code, not data) from the official CDN on first use;
  // cdn.jsdelivr.net = the verified, commit-pinned data host (only when the manifest names it)
  for (const h of external) expect(h, `unexpected external host ${h}`).toMatch(/(^|\.)openfreemap\.org$|^extensions\.duckdb\.org$|^(cdn|gcore)\.jsdelivr\.net$/);
  const joined = requests.join("\n");
  expect(joined).not.toMatch(/:8787|:8798|localhost:(?!4180)|192\.168\.|\b10\.\d+\.\d+\.\d+|:5433|postgres/i);
  // no /api request ever reached the network: the page answered them itself
  expect(requests.filter((u) => /\/api\//.test(u))).toEqual([]);
});

test("a feature can be selected and inspected", async ({ page }) => {
  await open(page);
  await page.evaluate(() => window.__gf.select(window.__gf.state.last!.geojson!.features[0]));
  const insp = page.locator("section.right");
  await expect(insp).toBeVisible();
  await expect(insp).toContainText("Đối tượng");
});

test("a real browser query runs: count_by over the places (aggregation, sorted, top 50)", async ({ page }) => {
  await open(page);
  const r = await page.evaluate(async () => {
    const res = await fetch("/api/query", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ source: "overture_places_hcm", kind: "count_by", group_by: "category", bbox: [106.66, 10.74, 106.74, 10.81], crs: "OGC:CRS84" }) });
    const plan = await res.json();
    const out = await window.__gf.resolvePlacement(plan);
    return { status: res.status, placement: plan.placement, rows: out.rows, total: out.rows_total };
  });
  expect(r.status).toBe(200);
  expect(r.placement).toBe("browser");
  expect(r.total).toBeGreaterThan(5);
  const counts = r.rows.map((x) => Number(x.count));
  expect(counts).toEqual([...counts].sort((a, b) => b - a));
});

test("a server-only analysis is refused clearly in both languages; the demo that needs it is disabled", async ({ page }) => {
  await open(page, { lang: "en" });
  const refused = await page.evaluate(async () => {
    const res = await fetch("/api/query", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ source: "overture_buildings_vn_south", kind: "near", bbox: [106.69, 10.77, 106.71, 10.79], crs: "OGC:CRS84", with: { source: "overture_transport_hcm", distance_m: 200 } }) });
    return { status: res.status, body: await res.json() };
  });
  expect(refused.status).toBe(422);
  expect(refused.body.error).toBe("server_required");
  expect(refused.body.message).toMatch(/requires GeoFabric Server/);
  await page.getByTestId("tab-query").click();
  await expect(page.getByTestId("demo-run-b")).toBeDisabled();
  await expect(page.getByTestId("demo-needs-server-b")).toContainText("This analysis requires GeoFabric Server.");
  await expect(page.getByTestId("demo-run-a")).toBeEnabled();
  await page.getByTestId("lang").selectOption("vi");
  await expect(page.getByTestId("demo-needs-server-b")).toContainText("Tính năng này cần GeoFabric Server.");
});

test("Developer mode states which engines this deployment has", async ({ page }) => {
  await open(page, { lang: "en", mode: "developer" });
  await page.getByTestId("tab-execution").click();
  await expect(page.getByTestId("planner-panel")).toBeVisible();
  await expect(page.getByTestId("engine-selected")).toHaveAttribute("data-engine", "duckdb-wasm");
  const alts = page.getByTestId("engine-alternative");
  await expect(alts).toHaveCount(2);
  await expect(alts.first()).toContainText("not available in this deployment");
  await expect(page.getByTestId("postgis-status")).toContainText("not configured");
});

test("Explorer stays plain: Vietnamese by default, no engine jargon, language switch works", async ({ page }) => {
  await open(page);
  await expect(page.getByTestId("lang")).toHaveValue("vi");
  const text = await page.locator("body").innerText();
  expect(text).not.toMatch(/PostGIS|DuckDB|PlanGraph|Wasm/i);
  await page.getByTestId("lang").selectOption("en");
  await expect(page.getByTestId("lang")).toHaveValue("en");
  await expect(page.getByTestId("mode-explorer")).toContainText(/Explor/);
});

test("the attribution page is published with the site and carries the ODbL notice", async ({ page }) => {
  await open(page);
  const r = await page.request.get("attribution.html");
  expect(r.status()).toBe(200);
  const body = await r.text();
  expect(body).toMatch(/Open Database License/);
  expect(body).toMatch(/OpenStreetMap contributors/);
  expect(body).toMatch(/Overture Maps Foundation/);
});

test("the basemap and its attribution load from the public origin", async ({ page }) => {
  await open(page, { tiles: "network" });
  await page.waitForFunction(() => window.__gf.basemapLoaded(), undefined, { timeout: 30_000 });
  await expect(page.locator("body")).toContainText(/OpenStreetMap/);
});

// ---- another layer, a layer that cannot be loaded, and what crosses the network ----
// The layers are turned on the way a user does it: the visibility checkbox of the data panel (English names: these tests run in English).
const showLayer = (page: Page, name: string) => page.getByLabel(`Show layer ${name}`).check();

test("a second layer (roads) is queried in the browser and drawn, with no server involved", async ({ page }) => {
  await open(page, { lang: "en" });
  await showLayer(page, "Roads");
  await page.waitForFunction(() => (window.__gf.contextInfo().overture_transport_hcm?.features ?? 0) > 0, undefined, { timeout: 90_000 });
  const n = await page.evaluate(() => window.__gf.contextInfo().overture_transport_hcm.features);
  expect(n).toBeGreaterThan(10);
});

test("a layer whose data cannot be loaded says so plainly; the map and the other layer keep working", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.route(/demo\/v1\/demo_places\.parquet/, (r) => r.abort());
  await open(page, { lang: "en" });
  await showLayer(page, "Places");
  await expect(page.getByTestId("status")).toContainText("could not be loaded", { timeout: 90_000 });
  expect(errors, "an unreachable layer is not an uncaught error").toEqual([]);
  // the building layer that is already on screen is untouched, and another layer still loads
  const still = await page.evaluate(() => window.__gf.state.last!.geojson!.features.length);
  expect(still).toBeGreaterThan(100);
  await showLayer(page, "Roads");
  await page.waitForFunction(() => (window.__gf.contextInfo().overture_transport_hcm?.features ?? 0) > 0, undefined, { timeout: 90_000 });
});

test("range reads: when the data host was verified for ranges, a query reads a small part of the file", async ({ page }) => {
  const pack = await (await page.request.get("demo/pack.json")).json() as { host: { range: boolean } | null; files: Record<string, { path: string; bytes: number }> };
  test.skip(!pack.host?.range, "this deployment serves whole files (host.range is false): range reads do not apply");
  const f = pack.files.overture_buildings_vn_south;
  const marker = f.path.split("/").pop()!;
  const seen: { method: string; range: boolean; status: number; bytes: number }[] = [];
  const pending: Promise<void>[] = [];
  page.on("response", (res) => {
    if (!res.url().includes(marker)) return;
    pending.push((async () => {
      const req = res.request();
      const sz = await req.sizes().catch(() => ({ responseBodySize: 0, responseHeadersSize: 0 }));
      seen.push({ method: req.method(), range: !!req.headers()["range"], status: res.status(), bytes: sz.responseBodySize + sz.responseHeadersSize });
    })());
  });
  await open(page);
  await new Promise((r) => setTimeout(r, 500));
  await Promise.all(pending);
  const gets = seen.filter((r) => r.method === "GET");
  const bytes = seen.reduce((a, r) => a + r.bytes, 0);
  expect(gets.length, "the layer was requested").toBeGreaterThan(0);
  expect(gets.every((r) => r.range && r.status === 206), `every data GET is a ranged 206: ${JSON.stringify(gets)}`).toBe(true);
  expect(bytes, `transferred ${bytes} of ${f.bytes} bytes`).toBeLessThan(f.bytes * 0.25);
});
