import { expect, test } from "@playwright/test";
import * as fs from "node:fs";

// Held-out planner evaluation (ADR-010). For every case: what the planner decides by default, then what each engine
// really costs (end to end, through the simulated network), so accuracy and regret can be computed offline
// (benchmarks/analyze_heldout.py). The gateway runs with a frozen calibration (--no-learn): the cases never teach it.
test.skip(!process.env.HELDOUT, "set HELDOUT=1 (see playwright.heldout.config.ts)");
test.setTimeout(60 * 60_000);

interface Case {
  id: string; dataset: string; crs: string; fraction: number; bbox: number[]; kind: string; limit: number | null;
  group_by: string | null; filter: unknown; deterministic: boolean; network: string; browser_state: "warm" | "cold";
}
interface GF {
  resolvePlacement(r: unknown): Promise<{ rows_total: number; result_digest?: { count: number; sha256: string } }>;
  wasm: { recycleBudget: number; recycle(): Promise<void>; isWarm(): boolean; instance(): Promise<unknown> };
}
declare global { interface Window { __gf: GF } }

const SAFE_MB = Number(process.env.HO_SAFE_MB ?? 220); // never run the browser on a plan whose estimated input exceeds this (tab memory)
const BROWSER_SAFE_INPUT = SAFE_MB * 1024 * 1024;
const median = (a: number[]) => [...a].sort((x, y) => x - y)[Math.floor(a.length / 2)];

type Cand = { engine: string; feasible: boolean; score_ms: number | null; rejected_because: string[]; confidence: { score: number } | null; risk: unknown; cost: unknown };

test("held-out: planner decision vs measured server/browser time", async ({ context }) => {
  const net = process.env.HO_NET!;
  const cases: Case[] = (JSON.parse(fs.readFileSync(process.env.HO_CASES!, "utf8")).cases as Case[]).filter((c) => c.network === net);
  expect(cases.length).toBeGreaterThan(0);
  const out = process.env.HO_OUT!;
  const rows: Record<string, unknown>[] = [];
  let page = await context.newPage();
  const open = async () => {
    await page.goto("/?e2e=1");
    await expect(page.locator("header .status")).toContainText("rows ·", { timeout: 120_000 });
    await page.waitForFunction(() => (window as unknown as { __gf: { state: { probe: unknown } } }).__gf.state.probe !== null, null, { timeout: 120_000 });
    await page.evaluate(() => { window.__gf.wasm.recycleBudget = Number.POSITIVE_INFINITY; });
  };
  const post = (p: string, b: unknown) =>
    page.evaluate(async ([pp, bb]) => {
      const t0 = performance.now();
      const r = await fetch(pp as string, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(bb) });
      const j = await r.json();
      return { status: r.status, json: j, ms: performance.now() - t0 };
    }, [p, b] as const);

  const byDataset = new Map<string, Case[]>();
  for (const c of cases) byDataset.set(c.dataset, [...(byDataset.get(c.dataset) ?? []), c]);
  for (const [dataset, list] of byDataset) {
    await page.close().catch(() => {});
    page = await context.newPage(); // fresh tab per dataset: a crash never poisons the next dataset
    await open();
    for (const c of list) {
      const req = { source: c.dataset, kind: c.kind, bbox: c.bbox, crs: c.crs, limit: c.limit ?? undefined, group_by: c.group_by ?? undefined, filter: c.filter ?? undefined };
      const row: Record<string, unknown> = { case: c, dataset };
      try {
        // browser state
        if (c.browser_state === "cold") await page.evaluate(() => window.__gf.wasm.recycle());
        else await page.evaluate(() => window.__gf.wasm.instance());
        const warm = await page.evaluate(() => window.__gf.wasm.isWarm());
        row.warm_at_plan = warm;
        // 1. the planner's own decision (default configuration, frozen calibration, simulated network profile)
        const plan = await post("/api/query/plan", { ...req, client: { warm } });
        if (plan.status !== 200) { row.plan_error = JSON.stringify(plan.json).slice(0, 300); rows.push(row); continue; }
        const ph = plan.json.physical;
        const planner = {
          engine: ph.engine, placement: ph.placement, confidence: ph.assessment.confidence.score, engine_confidence: ph.assessment.engine_confidence,
          margin_factor: ph.assessment.margin_factor, estimate: ph.assessment.estimate, policy: ph.assessment.policy, network: ph.assessment.network.name,
          candidates: (ph.candidates as Cand[]).map((k) => ({ engine: k.engine, feasible: k.feasible, score_ms: k.score_ms, rejected_because: k.rejected_because, confidence: k.confidence?.score ?? null, risk: k.risk, cost: k.cost })),
          result_rows_estimate: ph.result_rows_estimate, input_bytes: ph.input_bytes_upper_bound, browser_access: ph.assessment.browser_access as { strategy: string; requests_est: number; fetch_bytes_est: number; file_bytes: number }[],
          covering_bytes: ph.scans.reduce((a: number, s: { covering_bytes_total: number }) => a + s.covering_bytes_total, 0) as number,
          kept_bytes: ph.scans.reduce((a: number, s: { prune: { kept_bytes: number } }) => a + s.prune.kept_bytes, 0) as number,
        };
        row.planner = planner;
        // 2. server: warm-up + 2 timed
        const sT: number[] = [];
        let sDigest: unknown = null, sRows = -1;
        for (let i = 0; i < 3; i++) {
          const r = await post("/api/query", { ...req, engine: "duckdb", browser: false, client: { warm } });
          if (r.status !== 200) { row.server_error = JSON.stringify(r.json).slice(0, 300); break; }
          if (i > 0) sT.push(r.ms);
          sDigest = r.json.result_digest; sRows = r.json.rows_total;
        }
        row.server = { ms: sT, median: sT.length ? median(sT) : null, rows: sRows };
        // 3. browser (only when a tab can hold it)
        const engineInput = planner.browser_access.reduce((a, x) => a + x.fetch_bytes_est, 0); // bytes the browser would pull into the tab
        const b: Record<string, unknown> = { ms: [], rows: null, file_requests: [], file_bytes: [] };
        if (engineInput > BROWSER_SAFE_INPUT) {
          b.skipped = `browser fetch ${(engineInput / 1048576).toFixed(0)} MB > ${BROWSER_SAFE_INPUT / 1048576} MB (tab crash threshold)`;
        } else {
          const times: number[] = [];
          let bDigest: unknown = null;
          for (let i = 0; i < 3; i++) {
            const st0 = (await page.evaluate(() => fetch("/api/stats").then((r) => r.json()))) as { file_requests: number; file_bytes: number };
            const t0 = Date.now();
            const warmNow = await page.evaluate(() => window.__gf.wasm.isWarm());
            const placed = await post("/api/query", { ...req, engine: "duckdb-wasm", browser_limit_mb: SAFE_MB, client: { warm: warmNow } });
            if (placed.status !== 200 || placed.json.placement !== "browser") {
              b.not_placed = placed.status !== 200
                ? JSON.stringify(placed.json).slice(0, 200)
                : ((placed.json.physical?.candidates ?? []) as Cand[]).find((k) => k.engine === "duckdb-wasm")?.rejected_because?.join("; ") ?? "placed on the server";
              break;
            }
            try {
              const res = await page.evaluate((r) => window.__gf.resolvePlacement(r), placed.json);
              times.push(Date.now() - t0);
              const st1 = (await page.evaluate(() => fetch("/api/stats").then((r) => r.json()))) as { file_requests: number; file_bytes: number };
              (b.file_requests as number[] | undefined)?.push(st1.file_requests - st0.file_requests);
              (b.file_bytes as number[] | undefined)?.push(st1.file_bytes - st0.file_bytes);
              b.rows = res.rows_total; bDigest = res.result_digest;
            } catch (e) {
              b.error = String(e).slice(0, 300);
              if (String(e).includes("crashed")) {
                b.crashed = true;
                await page.close().catch(() => {});
                page = await context.newPage();
                await context.request.post("/api/state/reset"); // measurement runs: a crash must not block the next case (health cool-down); the page re-probes below
                await open();
              }
              break;
            }
          }
          b.ms = times;
          if (c.browser_state === "cold") { b.cold_ms = times[0] ?? null; b.median = times.length ? times[0] : null; }
          else b.median = times.length > 1 ? median(times.slice(1)) : (times[0] ?? null);
          if (bDigest && c.deterministic) b.digest_match = JSON.stringify(bDigest) === JSON.stringify(sDigest);
          if (b.rows != null && sRows >= 0) b.rows_match = b.rows === sRows;
        }
        row.browser = b;
      } catch (e) {
        row.harness_error = String(e).slice(0, 300);
        if (page.isClosed()) { page = await context.newPage(); await open(); }
      }
      rows.push(row);
      const p = row.planner as { engine?: string } | undefined;
      const s = row.server as { median: number | null } | undefined;
      const br = row.browser as { median?: number | null; skipped?: string; not_placed?: string; error?: string } | undefined;
      console.log(`${c.id} ${net} ${dataset} ${c.kind} f=${c.fraction} ${c.browser_state}: planner=${p?.engine} server=${s?.median?.toFixed(0)} browser=${br?.median != null ? br.median.toFixed(0) : (br?.skipped ?? br?.not_placed ?? br?.error)}`);
      fs.writeFileSync(out, JSON.stringify({ network: net, rows }, null, 1)); // incremental: a long run keeps what it has
    }
  }
  fs.writeFileSync(out, JSON.stringify({ network: net, rows }, null, 1));
  const bad = rows.filter((r) => {
    const b = r.browser as { digest_match?: boolean; rows_match?: boolean } | undefined;
    return b?.digest_match === false || b?.rows_match === false;
  });
  expect(bad.map((r) => (r.case as Case).id), "cross-engine mismatches").toEqual([]);
});
