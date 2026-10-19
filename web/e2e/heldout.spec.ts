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
  resolvePlacement(r: unknown): Promise<{ rows_total: number; result_digest?: { count: number; sha256: string }; phases?: unknown }>;
  wasm: { recycleBudget: number; recycle(): Promise<void>; isWarm(): boolean; instance(): Promise<unknown> };
}
declare global { interface Window { __gf: GF } }

const SAFE_MB = Number(process.env.HO_SAFE_MB ?? 220); // never run the browser on a plan whose estimated input exceeds this (tab memory)
const BROWSER_SAFE_INPUT = SAFE_MB * 1024 * 1024;
const median = (a: number[]) => [...a].sort((x, y) => x - y)[Math.floor(a.length / 2)];

// Chrome-level network throttling (DevTools protocol): the page, its workers (DuckDB-Wasm) and every fetch are throttled by the
// browser itself, independently of the gateway (HO_CDP=1). Numbers = the planner's profiles.
const CDP: Record<string, { latency: number; down: number; up: number }> = {
  FAST_LAN: { latency: 5, down: 1000e6 / 8, up: 1000e6 / 8 },
  GOOD_BROADBAND: { latency: 20, down: 200e6 / 8, up: 50e6 / 8 },
  MOBILE: { latency: 70, down: 30e6 / 8, up: 10e6 / 8 },
  HIGH_LATENCY: { latency: 150, down: 20e6 / 8, up: 5e6 / 8 },
};

type Cand = { engine: string; feasible: boolean; score_ms: number | null; rejected_because: string[]; confidence: { score: number } | null; risk: unknown; cost: unknown };

test("held-out: planner decision vs measured server/browser time", async ({ context }) => {
  const net = process.env.HO_NET!;
  const cases: Case[] = (JSON.parse(fs.readFileSync(process.env.HO_CASES!, "utf8")).cases as Case[]).filter((c) => c.network === net);
  expect(cases.length).toBeGreaterThan(0);
  const out = process.env.HO_OUT!;
  const rows: Record<string, unknown>[] = [];
  let page = await context.newPage();
  const open = async () => {
    await page.goto("/?e2e=1&mode=developer&lang=en&source=clustered_10mb_spatial");
    await expect(page.getByTestId("status")).toContainText("results", { timeout: 120_000 });
    await page.waitForFunction(() => (window as unknown as { __gf: { state: { probe: unknown } } }).__gf.state.probe !== null, null, { timeout: 120_000 });
    await page.evaluate(() => { window.__gf.wasm.recycleBudget = Number.POSITIVE_INFINITY; });
    const prof = CDP[net];
    if (process.env.HO_CDP && prof) {
      const cdp = await context.newCDPSession(page);
      await cdp.send("Network.enable");
      await cdp.send("Network.emulateNetworkConditions", { offline: false, latency: prof.latency, downloadThroughput: prof.down, uploadThroughput: prof.up });
    }
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
    // drift control: a fixed reference measured at the start of every dataset batch (same query, both engines), so the machine state of
    // this batch can be read off the result (identical cases re-measured hours apart differed by 0.56-1.68x on this host)
    const refReq = { source: "clustered_10mb_spatial", kind: "count", bbox: [100.5, 8.5, 109.5, 23.5], crs: "EPSG:4326" };
    const refS: number[] = [], refB: number[] = [];
    try {
      for (let i = 0; i < 5; i++) {
        const r = await post("/api/query", { ...refReq, engine: "duckdb", browser: false });
        if (r.status === 200) refS.push(r.ms);
        const o = await page.evaluate(async (rq) => {
          const t0p = performance.now();
          const resp = await fetch("/api/query", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ ...rq, engine: "duckdb-wasm", browser_limit_mb: 220, client: { warm: true } }) });
          const placed = await resp.json();
          if (!resp.ok || placed.placement !== "browser") return null;
          await window.__gf.resolvePlacement(placed);
          return performance.now() - t0p;
        }, refReq);
        if (o !== null) refB.push(o);
      }
    } catch { /* the reference is a control, never a reason to stop */ }
    const reference = { server_ms: refS.length ? median(refS) : null, browser_ms: refB.length > 1 ? median(refB.slice(1)) : null, at: new Date().toISOString() };
    for (const c of list) {
      const req = { source: c.dataset, kind: c.kind, bbox: c.bbox, crs: c.crs, limit: c.limit ?? undefined, group_by: c.group_by ?? undefined, filter: c.filter ?? undefined };
      const row: Record<string, unknown> = { case: c, dataset, reference };
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
        const b: Record<string, unknown> = { ms: [], outer_ms: [], rows: null, file_requests: [], file_bytes: [], plan_ms: [], resolve_ms: [], phases: [] };
        if (engineInput > BROWSER_SAFE_INPUT) {
          b.skipped = `browser fetch ${(engineInput / 1048576).toFixed(0)} MB > ${BROWSER_SAFE_INPUT / 1048576} MB (tab crash threshold)`;
        } else {
          const times: number[] = [];
          let bDigest: unknown = null;
          for (let i = 0; i < 3; i++) {
            const st0 = (await page.evaluate(() => fetch("/api/stats").then((r) => r.json()))) as { file_requests: number; file_bytes: number };
            const warmNow = await page.evaluate(() => window.__gf.wasm.isWarm());
            const t0 = Date.now();
            // TIMING CONTRACT (final planner validation): the browser time is measured INSIDE the page with one clock, from the moment the
            // placed request leaves to the moment the result exists in the tab: plan request + response (gateway planning included)
            // + engine start (cold only) + open + first batch + drain + conversion. It excludes the test driver's own round trips
            // (three CDP calls ~5-15 ms in the older harness) and anything the UI does afterwards (drawing). The planner's browser
            // model predicts exactly this quantity (crates/geo-planner/src/browser_model.rs, docs/claude/pre-postgis-gate.md).
            try {
            const out = await page.evaluate(async ([rq, safeMb, warmFlag]) => {
              const t0p = performance.now();
              const resp = await fetch("/api/query", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ ...(rq as object), engine: "duckdb-wasm", browser_limit_mb: safeMb, client: { warm: warmFlag } }) });
              const placed = await resp.json();
              const t1p = performance.now();
              if (!resp.ok || placed.placement !== "browser") {
                const k = ((placed.physical?.candidates ?? []) as { engine: string; rejected_because?: string[] }[]).find((x) => x.engine === "duckdb-wasm");
                return { placed: false as const, status: resp.status, body: resp.ok ? null : JSON.stringify(placed).slice(0, 200), why: k?.rejected_because?.join("; ") ?? "placed on the server", plan_ms: t1p - t0p };
              }
              const x = await window.__gf.resolvePlacement(placed);
              const t2p = performance.now();
              return { placed: true as const, plan_ms: t1p - t0p, resolve_ms: t2p - t1p, total: t2p - t0p, rows_total: x.rows_total, digest: x.result_digest, phases: x.phases ?? null };
            }, [req, SAFE_MB, warmNow] as const);
            if (!out.placed) {
              b.not_placed = out.status !== 200 ? (out.body ?? "") : out.why;
              break;
            }
              const res = out;
              times.push(Math.round(out.total * 10) / 10);
              (b.outer_ms as number[]).push(Date.now() - t0); // the older definition (includes the test driver's round trips), kept for comparison
              (b.plan_ms as number[]).push(Math.round(out.plan_ms * 10) / 10);
              (b.resolve_ms as number[]).push(Math.round(out.resolve_ms * 10) / 10);
              (b.phases as unknown[]).push(res.phases ?? null);
              const st1 = (await page.evaluate(() => fetch("/api/stats").then((r) => r.json()))) as { file_requests: number; file_bytes: number };
              (b.file_requests as number[] | undefined)?.push(st1.file_requests - st0.file_requests);
              (b.file_bytes as number[] | undefined)?.push(st1.file_bytes - st0.file_bytes);
              b.rows = res.rows_total; bDigest = res.digest;
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
