import { expect, test } from "@playwright/test";
import * as fs from "node:fs";
import * as path from "node:path";

// Crossover measurement: the same query on the server engine and on the browser engine, across
// dataset sizes and selectivities, with the planner's prediction recorded next to the measurement.
// Not part of the normal suite:   CROSSOVER=1 npx playwright test crossover
// Output: benchmarks/results/crossover-<tag>.json  (+ digests compared whenever the result is deterministic)
//
// Caveat (stated in the report): the browser and the gateway share this machine, so network transfer
// is ~free here; the planner's network parameter is an assumption, not a measurement.

test.skip(!process.env.CROSSOVER, "set CROSSOVER=1 to run the (slow) crossover measurement");
test.setTimeout(30 * 60_000);

interface PlanResp {
  placement: string;
  physical: {
    engine: string;
    input_bytes_upper_bound: number;
    result_rows_estimate: number;
    candidates: { engine: string; feasible: boolean; score_ms: number | null; rejected_because: string[] }[];
  };
}
interface GF {
  resolvePlacement(r: unknown): Promise<{ rows_total: number; result_digest?: { count: number; sha256: string } }>;
}
declare global {
  interface Window { __gf: GF }
}

const DATASETS: { id: string; crs: string; extent: [number, number, number, number]; cat: string; focus: [number, number] }[] = [
  { id: "clustered_10mb_spatial", crs: "EPSG:4326", extent: [100, 8, 110, 24], cat: "category", focus: [104.5, 12.5] },
  { id: "clustered_100mb_spatial", crs: "EPSG:4326", extent: [100, 8, 110, 24], cat: "category", focus: [104.5, 12.5] },
  { id: "clustered_1gb_spatial", crs: "EPSG:4326", extent: [100, 8, 110, 24], cat: "category", focus: [104.5, 12.5] },
  { id: "overture_buildings_vn_south", crs: "OGC:CRS84", extent: [105, 8.5, 108, 12], cat: "version", focus: [106.7, 10.8] },
];
// window = fraction of the dataset extent area, centred on a dense focus point
const FRACTIONS = [0.002, 0.01, 0.05, 0.25];
const KINDS: { kind: string; limit?: number; deterministic: boolean }[] = [
  { kind: "count_by", deterministic: true }, // tiny result: input-bound
  { kind: "features", limit: 2000, deterministic: false }, // medium result
  { kind: "features", limit: 50000, deterministic: false }, // large result: transfer-bound
];
const BROWSER_SAFE_INPUT = 700 * 1024 * 1024; // do not try the browser above this (tab memory)

function window_(e: [number, number, number, number], f: number, focus: [number, number]) {
  const w = (e[2] - e[0]) * Math.sqrt(f), h = (e[3] - e[1]) * Math.sqrt(f);
  const cx = Math.min(Math.max(focus[0], e[0] + w / 2), e[2] - w / 2), cy = Math.min(Math.max(focus[1], e[1] + h / 2), e[3] - h / 2);
  return [cx - w / 2, cy - h / 2, cx + w / 2, cy + h / 2];
}

const median = (a: number[]) => [...a].sort((x, y) => x - y)[Math.floor(a.length / 2)];

test("server vs browser: time, planner prediction and result agreement", async ({ context }) => {
  let page = await context.newPage();
  const open = async () => {
    await page.goto("/?e2e=1");
    await expect(page.locator("header .status")).toContainText("rows ·", { timeout: 60_000 });
  };
  await open();
  const rows: Record<string, unknown>[] = [];

  const post = (p: string, b: unknown) =>
    page.evaluate(async ([pp, bb]) => {
      const t0 = performance.now();
      const r = await fetch(pp as string, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(bb) });
      const j = await r.json();
      return { status: r.status, json: j, ms: performance.now() - t0 };
    }, [p, b] as const);

  // warm both engines once (wasm instantiation is a one-off cost; reported separately)
  const warmReq = { source: DATASETS[0].id, kind: "count_by", group_by: DATASETS[0].cat, bbox: [104, 12, 105, 13], crs: DATASETS[0].crs };
  const t0 = Date.now();
  const warmPlan = await post("/api/query", { ...warmReq, engine: "duckdb-wasm" });
  await page.evaluate((r) => window.__gf.resolvePlacement(r), warmPlan.json);
  const wasmColdMs = Date.now() - t0;
  await post("/api/query", { ...warmReq, engine: "duckdb", browser: false });

  for (const ds of DATASETS) {
    // clean timings: a fresh tab per dataset and no recycling inside it (recycling adds seconds of
    // re-instantiation to whichever case triggers it; the crash it prevents needs several large files in one tab)
    await page.close().catch(() => {});
    page = await context.newPage();
    await open();
    await page.evaluate(() => { (window as unknown as { __gf: { wasm: { recycleBudget: number } } }).__gf.wasm.recycleBudget = Number.POSITIVE_INFINITY; });
    for (const f of FRACTIONS) {
      for (const k of KINDS) {
        const bbox = window_(ds.extent, f, ds.focus);
        const req = { source: ds.id, kind: k.kind, bbox, crs: ds.crs, limit: k.limit, group_by: k.kind === "count_by" ? ds.cat : undefined };
        const auto = await post("/api/query/plan", req);
        if (auto.status !== 200) { rows.push({ dataset: ds.id, fraction: f, kind: k.kind, limit: k.limit, error: JSON.stringify(auto.json).slice(0, 200) }); continue; }
        const plan = auto.json as PlanResp;
        const input = plan.physical.input_bytes_upper_bound;
        const row: Record<string, unknown> = {
          dataset: ds.id, fraction: f, kind: k.kind, limit: k.limit ?? null, input_bytes_est: input, result_rows_est: plan.physical.result_rows_estimate,
          planner_selected: plan.physical.engine,
          planner_scores: Object.fromEntries(plan.physical.candidates.map((c) => [c.engine, c.feasible ? c.score_ms : `rejected: ${c.rejected_because.join("; ")}`])),
        };
        // server: 1 warm-up + 3 timed
        const sTimes: number[] = [];
        let sDigest: unknown = null, sRows = 0;
        for (let i = 0; i < 4; i++) {
          const r = await post("/api/query", { ...req, engine: "duckdb", browser: false });
          if (r.status !== 200) { row.server_error = JSON.stringify(r.json).slice(0, 200); break; }
          if (i > 0) sTimes.push(r.ms);
          sDigest = r.json.result_digest; sRows = r.json.rows_total;
        }
        row.server_ms = sTimes.length ? median(sTimes) : null;
        row.server_rows = sRows;
        // browser: skipped when the planner-estimated input exceeds what a tab can hold
        if (input > BROWSER_SAFE_INPUT) {
          row.browser_skipped = `estimated input ${input} bytes > ${BROWSER_SAFE_INPUT}`;
        } else {
          const bTimes: number[] = [];
          let bDigest: unknown = null, bRows = 0;
          for (let i = 0; i < 4; i++) {
            const t = Date.now();
            const planned = await post("/api/query", { ...req, engine: "duckdb-wasm", browser: true });
            if (planned.status !== 200 || planned.json.placement !== "browser") { row.browser_not_placed = "the planner kept the plan on the server (hard limit); see planner_scores"; bTimes.length = 0; break; }
            try {
              const res = await page.evaluate((r) => window.__gf.resolvePlacement(r), planned.json);
              if (i > 0) bTimes.push(Date.now() - t);
              bDigest = res.result_digest; bRows = res.rows_total;
            } catch (e) {
              row.browser_error = String(e).slice(0, 200);
              if (String(e).includes("crashed")) {
                // a crashed tab is a result, not a test failure: record it and continue in a fresh tab
                row.browser_crashed = true;
                await page.close().catch(() => {});
                page = await context.newPage();
                await open();
              }
              break;
            }
          }
          row.browser_ms = bTimes.length ? median(bTimes) : null;
          row.browser_rows = bRows;
          if (!row.browser_not_placed && !row.browser_error) {
            if (k.deterministic) row.digest_match = JSON.stringify(sDigest) === JSON.stringify(bDigest);
            else row.rows_match = sRows === bRows;
          }
        }
        rows.push(row);
        console.log(JSON.stringify(row));
      }
    }
  }
  const out = path.resolve("..", "benchmarks", "results", `crossover-${process.env.CROSSOVER_TAG ?? "run"}.json`);
  fs.writeFileSync(out, JSON.stringify({ wasm_cold_start_ms: wasmColdMs, rows }, null, 2));
  const bad = rows.filter((r) => r.digest_match === false || r.rows_match === false);
  expect(bad, `cross-engine mismatches: ${JSON.stringify(bad)}`).toEqual([]);
});
