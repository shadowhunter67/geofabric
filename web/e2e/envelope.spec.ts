import { expect, test } from "@playwright/test";
import * as fs from "node:fs";

// Browser safe-operating-envelope measurement (ADR-010): the browser engine (range requests, Chrome/Edge) on sources of 1 GB+,
// with the amount of data it has to pull growing step by step, until it fails. Each run is a fresh tab.
//   ENVELOPE=1 EN_OUT=../benchmarks/results/envelope-chrome.json npx playwright test -c playwright.heldout.config.ts envelope
test.skip(!process.env.ENVELOPE, "set ENVELOPE=1");
test.setTimeout(90 * 60_000);

interface Run { source: string; kind: string; fraction: number; limit: number | null; layout: string }
const SYN = [100, 8, 110, 24], OV = [105, 8.5, 108, 12];
function window_(e: number[], f: number, focus: number[]) {
  const w = (e[2] - e[0]) * Math.sqrt(f), h = (e[3] - e[1]) * Math.sqrt(f);
  const cx = Math.min(Math.max(focus[0], e[0] + w / 2), e[2] - w / 2), cy = Math.min(Math.max(focus[1], e[1] + h / 2), e[3] - h / 2);
  return [cx - w / 2, cy - h / 2, cx + w / 2, cy + h / 2];
}
const FR = [0.01, 0.05, 0.1, 0.25, 0.5, 1.0];
const runs: Run[] = [];
for (const f of FR) runs.push({ source: "clustered_1gb_spatial", kind: "count_by", fraction: f, limit: null, layout: "sorted" });
for (const f of [0.05, 0.25, 1.0]) runs.push({ source: "clustered_1gb_spatial", kind: "features", fraction: f, limit: 100000, layout: "sorted" });
for (const f of [0.1, 0.5, 1.0]) runs.push({ source: "clustered_1gb_shuffled", kind: "count_by", fraction: f, limit: null, layout: "shuffled (no pruning)" });
for (const f of [0.1, 1.0]) runs.push({ source: "clustered_1gb_shuffled", kind: "features", fraction: f, limit: 100000, layout: "shuffled (no pruning)" });
for (const f of FR) runs.push({ source: "overture_buildings_vn_south", kind: "count_by", fraction: f, limit: null, layout: "Overture (real)" });
for (const f of [0.01, 0.25, 1.0]) runs.push({ source: "overture_buildings_vn_south", kind: "features", fraction: f, limit: 100000, layout: "Overture (real)" });

test("browser envelope", async ({ browser }) => {
  const out: Record<string, unknown>[] = [];
  for (const r of runs) {
    const context = await browser.newContext();
    let page = await context.newPage();
    const row: Record<string, unknown> = { ...r, browser: browser.browserType().name() + " " + browser.version() };
    try {
      await page.goto("/?e2e=1&mode=developer&lang=en&source=clustered_10mb_spatial");
      await expect(page.getByTestId("status")).toContainText("results", { timeout: 120_000 });
      await page.waitForFunction(() => (window as unknown as { __gf: { state: { probe: unknown } } }).__gf.state.probe !== null, null, { timeout: 120_000 });
      const ov = r.source.startsWith("overture");
      const bbox = ov ? window_(OV, r.fraction, [106.7, 10.8]) : window_(SYN, r.fraction, [104.5, 12.5]);
      const req = { source: r.source, kind: r.kind, bbox, crs: ov ? "OGC:CRS84" : "EPSG:4326", limit: r.limit ?? undefined, group_by: r.kind === "count_by" ? (ov ? "version" : "category") : undefined };
      const post = (p: string, b: unknown) => page.evaluate(async ([pp, bb]) => { const t0 = performance.now(); const x = await fetch(pp as string, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(bb) }); return { status: x.status, json: await x.json(), ms: performance.now() - t0 }; }, [p, b] as const);
      const plan = await post("/api/query/plan", req);
      const ba = plan.json.physical.assessment.browser_access as { fetch_bytes_est: number; requests_est: number; strategy: string }[];
      row.planner_fetch_est_mb = ba.reduce((a, x) => a + x.fetch_bytes_est, 0) / 1048576;
      row.planner_requests_est = ba.reduce((a, x) => a + x.requests_est, 0);
      row.kept_mb = plan.json.physical.scans.reduce((a: number, s: { prune: { kept_bytes: number } }) => a + s.prune.kept_bytes, 0) / 1048576;
      const s0 = await page.evaluate(() => fetch("/api/stats").then((x) => x.json()));
      const placed = await post("/api/query", { ...req, engine: "duckdb-wasm", browser_limit_mb: 100000 });
      if (placed.json.placement !== "browser") { row.outcome = "not_placed"; row.detail = JSON.stringify(placed.json.physical?.candidates?.[1]?.rejected_because ?? placed.json).slice(0, 200); out.push(row); await context.close(); continue; }
      const t0 = Date.now();
      try {
        const res = await page.evaluate((p) => (window as unknown as { __gf: { resolvePlacement(r: unknown): Promise<{ rows_total: number }> } }).__gf.resolvePlacement(p), placed.json);
        row.outcome = "ok"; row.ms = Date.now() - t0; row.rows = res.rows_total;
        const heap = await page.evaluate(() => (performance as unknown as { memory?: { usedJSHeapSize: number } }).memory?.usedJSHeapSize ?? null);
        row.js_heap_mb = heap ? heap / 1048576 : null;
      } catch (e) {
        row.outcome = String(e).includes("crashed") ? "crash" : String(e).includes("timeout") ? "timeout" : "error"; row.ms = Date.now() - t0; row.detail = String(e).slice(0, 200);
      }
      const s1 = await context.request.get("/api/stats").then((x) => x.json());
      row.fetched_mb = (s1.file_bytes - s0.file_bytes) / 1048576; row.requests = s1.file_requests - s0.file_requests;
      await context.request.post("/api/state/reset");
    } catch (e) { row.outcome = "harness_error"; row.detail = String(e).slice(0, 200); }
    out.push(row);
    console.log(`${r.source} ${r.kind} f=${r.fraction} lim=${r.limit}: ${row.outcome} fetched=${(row.fetched_mb as number | undefined)?.toFixed?.(0)}MB est=${(row.planner_fetch_est_mb as number | undefined)?.toFixed?.(0)}MB ${row.ms ?? ""}ms`);
    fs.writeFileSync(process.env.EN_OUT!, JSON.stringify({ rows: out }, null, 1));
    await context.close();
  }
});
