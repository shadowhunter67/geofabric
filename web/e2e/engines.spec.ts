import { expect, test } from "@playwright/test";
import { openApp, tab } from "./helpers.ts";

// Cross-engine tests: the SAME logical plan (IR 0.2) runs on the server engine (DuckDB, via the gateway)
// and on the browser engine (DuckDB-Wasm, in the page) and must produce the same canonical digest.
// Needs: `npm run build`, benchmarks/data (10mb files).

interface Resp {
  placement: string;
  result_digest?: { count: number; sha256: string };
  physical: { engine: string; placement: string; candidates: { engine: string; feasible: boolean; score_ms: number | null; rejected_because: string[] }[] };
  rows_total: number;
  metrics?: { bytes_read: { value: number | null; source: string } };
}
interface GF {
  resolvePlacement(r: unknown): Promise<Resp>;
}
declare global {
  interface Window { __gf: GF }
}

const BBOX = [104.0, 12.0, 105.0, 13.0];
const q = (extra: Record<string, unknown>) => ({ source: "clustered_10mb_spatial", kind: "features", bbox: BBOX, crs: "EPSG:4326", limit: 100000, ...extra });

async function post(page: import("@playwright/test").Page, path: string, body: unknown) {
  return page.evaluate(async ([p, b]) => {
    const r = await fetch(p as string, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(b) });
    return { status: r.status, json: await r.json() };
  }, [path, body] as const);
}

const recorded: { query: string; duckdb: unknown; wasm: unknown; match: boolean; rows: number }[] = [];
test.afterAll(async () => {
  // evidence for the report: HASHES_OUT=<file> npx playwright test engines
  if (process.env.HASHES_OUT) (await import("node:fs")).writeFileSync(process.env.HASHES_OUT, JSON.stringify(recorded, null, 2));
});

/** Run one query template on both engines; return both digests (browser one computed in the page). */
async function both(page: import("@playwright/test").Page, req: Record<string, unknown>, label = "") {
  const server = await post(page, "/api/query", { ...req, engine: "duckdb", browser: false });
  expect(server.status, JSON.stringify(server.json).slice(0, 300)).toBe(200);
  const planned = await post(page, "/api/query", { ...req, engine: "duckdb-wasm" });
  expect(planned.status, JSON.stringify(planned.json).slice(0, 300)).toBe(200);
  expect(planned.json.placement).toBe("browser");
  const browser = await page.evaluate((r) => window.__gf.resolvePlacement(r), planned.json);
  const sr = server.json as Resp, br = browser as Resp;
  recorded.push({ query: label, duckdb: sr.result_digest, wasm: br.result_digest, match: JSON.stringify(sr.result_digest) === JSON.stringify(br.result_digest), rows: br.rows_total });
  return { server: sr, browser: br };
}

test.beforeEach(async ({ page }) => {
  await openApp(page);
});

test("same plan, two engines, same result: features in a viewport", async ({ page }) => {
  const { server, browser } = await both(page, q({}), "features in viewport (bbox, limit 100000)");
  expect(browser.placement).toBe("browser");
  expect(server.placement).toBe("server");
  expect(browser.rows_total).toBeGreaterThan(0);
  expect(browser.result_digest).toEqual(server.result_digest);
  expect(browser.metrics!.bytes_read).toEqual({ value: null, source: "unknown" });
});

test("same plan, two engines, same result: aggregate (count by category)", async ({ page }) => {
  const { server, browser } = await both(page, q({ kind: "count_by", group_by: "category", limit: undefined }), "count by category in viewport (aggregate)");
  expect(browser.result_digest).toEqual(server.result_digest);
  expect(browser.rows_total).toBe(server.rows_total);
});

test("same plan, two engines, same result: attribute filter + features", async ({ page }) => {
  const { server, browser } = await both(page, q({ filter: { column: "category", op: "eq", value: "school" } }), "features, category = school (attribute filter)");
  expect(browser.result_digest).toEqual(server.result_digest);
});

test("unsupported operation is never sent to the browser: planner falls back to DuckDB", async ({ page }) => {
  const r = await post(page, "/api/query", q({ kind: "intersects_features", engine: "duckdb-wasm" }));
  expect(r.status).toBe(200);
  expect(r.json.placement).toBe("server");
  const wasm = r.json.physical.candidates.find((c: { engine: string }) => c.engine === "duckdb-wasm");
  expect(wasm.feasible).toBe(false);
  expect(wasm.rejected_because.join(" ")).toContain("intersects");
});

test("browser disabled -> DuckDB", async ({ page }) => {
  const r = await post(page, "/api/query", q({ engine: "duckdb-wasm", browser: false }));
  expect(r.json.placement).toBe("server");
  const wasm = r.json.physical.candidates.find((c: { engine: string }) => c.engine === "duckdb-wasm");
  expect(wasm.rejected_because.join(" ")).toContain("disabled");
});

test("UI: choosing the browser engine runs the query locally and shows the planner decision", async ({ page }) => {
  await tab(page, "query").click();
  await page.getByTestId("q-where").selectOption("duckdb-wasm");
  await page.getByTestId("q-run").click();
  await expect(page.getByTestId("status")).toContainText("duckdb-wasm", { timeout: 60_000 });
  await tab(page, "execution").click();
  const panel = page.getByTestId("planner-panel");
  await expect(panel).toBeVisible();
  await expect(panel.getByTestId("engine-selected")).toHaveAttribute("data-engine", "duckdb-wasm");
  await expect(panel).toContainText("Local (browser)");
  await expect(panel.getByTestId("engine-alternative")).toHaveAttribute("data-engine", "duckdb");
  await expect(panel).toContainText("Estimated transfer to the browser");
  // results are real: table rows and a digest line
  await tab(page, "results").click();
  expect(await page.locator(".tabbody table tbody tr").count()).toBeGreaterThan(0);
});

test("UI: auto mode shows the selected engine, the other one as the alternative, and the reason", async ({ page }) => {
  await tab(page, "execution").click();
  const panel = page.getByTestId("planner-panel");
  // which engine wins is the planner's call (on a small file and a local link the browser is measured faster); both must be shown
  const selected = await panel.getByTestId("engine-selected").getAttribute("data-engine");
  expect(["duckdb", "duckdb-wasm"]).toContain(selected);
  await expect(panel.getByTestId("engine-alternative")).toHaveAttribute("data-engine", selected === "duckdb" ? "duckdb-wasm" : "duckdb");
  await expect(panel).toContainText("Estimated input");
});

test("UI: when the browser engine fails, the query is re-run on the server and the fallback is shown (failure injected)", async ({ page }) => {
  // inject a failure into the browser engine (the real-world triggers are exercised by the crossover run: unsupported
  // Parquet geometry types, tab crashes); this test covers the fallback mechanics and the honesty of the UI
  // the runtime probe also runs queries in the browser engine: let it finish first, or the injected failure is taken for an unfit runtime
  // and the planner (rightly) never tries the browser (flake investigation 2026-10-19: 25 % of runs)
  await page.waitForFunction(() => (window as unknown as { __gf: { state: { probe: unknown } } }).__gf.state.probe !== null, null, { timeout: 120_000 });
  await page.evaluate(() => {
    (window as unknown as { __gf: { wasm: { execute: () => Promise<never> } } }).__gf.wasm.execute = () => Promise.reject(new Error("injected browser failure"));
  });
  await tab(page, "query").click();
  await page.getByTestId("q-where").selectOption("duckdb-wasm");
  await page.getByTestId("q-run").click();
  await tab(page, "execution").click();
  const panel = page.getByTestId("planner-panel");
  // wait for the fallback itself: the status line already says "duckdb" before this run when the planner's default is the browser
  await expect(panel.getByTestId("engine-fallback")).toContainText("injected browser failure", { timeout: 30_000 });
  await expect(panel.getByTestId("engine-selected")).toHaveAttribute("data-engine", "duckdb");
  await expect(panel.getByTestId("engine-fallback")).toContainText("re-run once on the server");
});

test("real Overture subset (1.45 GB, DuckDB-typed GEOMETRY): browser range-request execution equals the server (digests)", async ({ page }) => {
  const req = { source: "overture_buildings_vn_south", kind: "count_by", group_by: "version", bbox: [106.64, 10.72, 106.76, 10.88], crs: "OGC:CRS84" };
  const probe = await post(page, "/api/query/plan", req);
  test.skip(probe.status === 404, "Overture subset not present");
  const { server, browser } = await both(page, req, "Overture count by version (range requests, typed geometry)");
  expect(browser.placement).toBe("browser");
  expect(browser.result_digest).toEqual(server.result_digest);
  // features: the geometry column is streamed (the bug fixed on 2026-10-14: typed GEOMETRY(crs) could not be streamed)
  const f = await both(page, { ...req, kind: "features", group_by: undefined, limit: 500 }, "Overture features limit 500 (geometry streamed as WKB)");
  expect(f.browser.rows_total).toBe(f.server.rows_total);
});
