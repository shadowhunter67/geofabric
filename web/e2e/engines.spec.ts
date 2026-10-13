import { expect, test } from "@playwright/test";

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

/** Run one query template on both engines; return both digests (browser one computed in the page). */
async function both(page: import("@playwright/test").Page, req: Record<string, unknown>) {
  const server = await post(page, "/api/query", { ...req, engine: "duckdb", browser: false });
  expect(server.status, JSON.stringify(server.json).slice(0, 300)).toBe(200);
  const planned = await post(page, "/api/query", { ...req, engine: "duckdb-wasm" });
  expect(planned.status, JSON.stringify(planned.json).slice(0, 300)).toBe(200);
  expect(planned.json.placement).toBe("browser");
  const browser = await page.evaluate((r) => window.__gf.resolvePlacement(r), planned.json);
  return { server: server.json as Resp, browser: browser as Resp };
}

test.beforeEach(async ({ page }) => {
  await page.goto("/?e2e=1");
  await expect(page.locator("header .status")).toContainText("rows ·", { timeout: 30_000 });
});

test("same plan, two engines, same result: features in a viewport", async ({ page }) => {
  const { server, browser } = await both(page, q({}));
  expect(browser.placement).toBe("browser");
  expect(server.placement).toBe("server");
  expect(browser.rows_total).toBeGreaterThan(0);
  expect(browser.result_digest).toEqual(server.result_digest);
  expect(browser.metrics!.bytes_read).toEqual({ value: null, source: "unknown" });
});

test("same plan, two engines, same result: aggregate (count by category)", async ({ page }) => {
  const { server, browser } = await both(page, q({ kind: "count_by", group_by: "category", limit: undefined }));
  expect(browser.result_digest).toEqual(server.result_digest);
  expect(browser.rows_total).toBe(server.rows_total);
});

test("same plan, two engines, same result: attribute filter + features", async ({ page }) => {
  const { server, browser } = await both(page, q({ filter: { column: "category", op: "eq", value: "school" } }));
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
