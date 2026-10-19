/**
 * Device check: a short, representative matrix of queries that tells whether the planner's browser/server decisions are sane on
 * THIS device (another laptop, another browser engine). It does not need Playwright: open
 *
 *     http://<gateway host>:8787/?devicecheck=1
 *
 * on the device, wait for "done", and send/copy the JSON. Start the gateway with `--devicecheck-dir <dir>` to have the result saved
 * on the server automatically (the endpoint does not exist otherwise).
 *
 * Safety: the check never forces the browser engine past the planner's safety envelope. Each case asks the planner; the browser
 * is run only if the planner offers it (the unsafe case MUST be refused: that is part of what is checked). Results are posted
 * after every case, so a tab crash leaves evidence of the last case.
 */

import type { RunResp } from "./types.ts";

export interface DeviceCase {
  id: string;
  /** what the case is for */
  why: string;
  req: Record<string, unknown>;
  /** the planner must NOT choose the browser for this case */
  mustNotUseBrowser?: boolean;
}

const HCM = [106.55, 10.65, 106.85, 10.9];
const SYN = [100.0, 8.0, 110.0, 24.0];
function win(e: number[], f: number, fx: number, fy: number): number[] {
  const w = (e[2] - e[0]) * Math.sqrt(f), h = (e[3] - e[1]) * Math.sqrt(f);
  const cx = Math.min(Math.max(e[0] + fx * (e[2] - e[0]), e[0] + w / 2), e[2] - w / 2);
  const cy = Math.min(Math.max(e[1] + fy * (e[3] - e[1]), e[1] + h / 2), e[3] - h / 2);
  return [cx - w / 2, cy - h / 2, cx + w / 2, cy + h / 2].map((x) => Math.round(x * 1e6) / 1e6);
}

export const DEVICE_MATRIX: DeviceCase[] = [
  { id: "small-local-count", why: "small query on a small file (whole-file mode)", req: { source: "clustered_10mb_spatial", kind: "count", bbox: win(SYN, 0.01, 0.3, 0.4), crs: "EPSG:4326" } },
  { id: "medium-features", why: "medium result on a 30 MB file", req: { source: "clustered_30mb_spatial", kind: "features", limit: 1500, bbox: win(SYN, 0.05, 0.5, 0.5), crs: "EPSG:4326" } },
  { id: "range-features-small", why: "range-request mode, tiny window on a 100 MB file", req: { source: "clustered_100mb_spatial", kind: "features", limit: 300, bbox: win(SYN, 0.002, 0.4, 0.6), crs: "EPSG:4326" } },
  { id: "range-heavy-count-by", why: "range-request mode, many row groups touched (latency amplification)", req: { source: "clustered_100mb_shuffled", kind: "count_by", group_by: "category", bbox: win(SYN, 0.2, 0.5, 0.5), crs: "EPSG:4326" } },
  { id: "large-result", why: "large result (40,000 rows) from a 30 MB file", req: { source: "clustered_30mb_spatial", kind: "features", limit: 40000, bbox: win(SYN, 0.5, 0.5, 0.5), crs: "EPSG:4326" } },
  { id: "geometry-roads", why: "real line geometry, attribute columns", req: { source: "overture_transport_hcm", kind: "features", limit: 2000, bbox: HCM, crs: "OGC:CRS84" } },
  { id: "places-count-by", why: "real points, aggregate", req: { source: "overture_places_hcm", kind: "count_by", group_by: "category", bbox: HCM, crs: "OGC:CRS84" } },
  { id: "unsafe-candidate", why: "1.45 GB file, huge result: the planner must keep it on the server", req: { source: "overture_buildings_vn_south", kind: "features", limit: 100000, bbox: [105.5, 9.5, 107.5, 11.5], crs: "OGC:CRS84" }, mustNotUseBrowser: true },
];

export interface DeviceCaseResult {
  id: string;
  why: string;
  status: "ok" | "skipped" | "error";
  note?: string;
  planner?: { engine: string; placement: string; confidence: number | null; server_ms: number | null; browser_ms: number | null; browser_safety: string | null; browser_feasible: boolean };
  server_ms?: number[];
  browser_ms?: number[];
  browser_phases?: unknown;
  rows?: { server: number | null; browser: number | null };
  digests_match?: boolean | null;
  browser_refused_correctly?: boolean;
}

export interface DeviceReport {
  started: string;
  device: { userAgent: string; platform: string; deviceMemoryGB: number | null; cores: number | null; screen: string; languages: string[] };
  link: { rtt_ms_median: number | null; down_mbps_estimate: number | null };
  probe: unknown;
  results: DeviceCaseResult[];
  done: boolean;
}

export interface DeviceDeps {
  post: <T>(path: string, body: unknown) => Promise<T>;
  /** run a plan response that was placed in the browser; returns it as server-shaped */
  resolve: (r: RunResp) => Promise<RunResp>;
  probe: () => unknown;
  /** forget the browser engine instance (cold start) */
  recycle: () => Promise<void>;
  report: (partial: DeviceReport) => Promise<void>;
  log: (m: string) => void;
}

const median = (v: number[]) => { const s = [...v].sort((a, b) => a - b); return s.length ? s[Math.floor(s.length / 2)] : null; };

async function measureLink(): Promise<DeviceReport["link"]> {
  const rtts: number[] = [];
  for (let i = 0; i < 9; i++) {
    const t0 = performance.now();
    await fetch("/api/health", { cache: "no-store" });
    rtts.push(performance.now() - t0);
  }
  // a 4 MB range of a data file: a rough throughput figure (not a benchmark)
  let down: number | null = null;
  try {
    const t0 = performance.now();
    const r = await fetch("/api/files/clustered_10mb_spatial", { headers: { range: "bytes=0-4194303" }, cache: "no-store" });
    const n = (await r.arrayBuffer()).byteLength;
    const s = (performance.now() - t0) / 1000;
    down = s > 0 ? (n * 8) / 1e6 / s : null;
  } catch { /* leave null */ }
  return { rtt_ms_median: median(rtts), down_mbps_estimate: down };
}

export async function runDeviceCheck(deps: DeviceDeps): Promise<DeviceReport> {
  const nav = navigator as Navigator & { deviceMemory?: number };
  const report: DeviceReport = {
    started: new Date().toISOString(),
    device: { userAgent: nav.userAgent, platform: nav.platform, deviceMemoryGB: nav.deviceMemory ?? null, cores: nav.hardwareConcurrency ?? null, screen: `${screen.width}x${screen.height}`, languages: [...(nav.languages ?? [])] },
    link: await measureLink(),
    probe: deps.probe(),
    results: [],
    done: false,
  };
  await deps.report(report);
  let first = true;
  for (const c of DEVICE_MATRIX) {
    const res: DeviceCaseResult = { id: c.id, why: c.why, status: "ok" };
    try {
      deps.log(`device check: ${c.id}`);
      if (first) { await deps.recycle(); first = false; } // the first browser run is a cold start
      // 1. what the planner would do, and its predictions
      const plan = await deps.post<{ physical: { engine: string; placement: string; assessment: { confidence: { score: number } }; candidates: { engine: string; feasible: boolean; score_ms: number | null; risk: { safety: string } | null }[] } }>("/api/query/plan", { ...c.req, client: { warm: false } });
      const cands = plan.physical.candidates;
      const srv = cands.find((k) => k.engine === "duckdb"), brw = cands.find((k) => k.engine === "duckdb-wasm");
      res.planner = {
        engine: plan.physical.engine, placement: plan.physical.placement, confidence: plan.physical.assessment.confidence.score ?? null,
        server_ms: srv?.score_ms ?? null, browser_ms: brw?.score_ms ?? null, browser_safety: brw?.risk?.safety ?? null, browser_feasible: !!brw?.feasible,
      };
      if (c.mustNotUseBrowser) {
        // ask for the browser explicitly (plan only, nothing runs): the planner must still refuse it
        const forced = await deps.post<{ physical: { placement: string } }>("/api/query/plan", { ...c.req, engine: "duckdb-wasm", client: { warm: true } });
        res.browser_refused_correctly = plan.physical.placement !== "browser" && forced.physical.placement !== "browser";
      }
      // 2. server, twice (first = warm-up)
      const sT: number[] = [];
      let sRows: number | null = null, sDigest: string | null = null;
      for (let i = 0; i < 2; i++) {
        const t0 = performance.now();
        const r = await deps.post<RunResp>("/api/query", { ...c.req, engine: "duckdb", browser: false });
        sT.push(performance.now() - t0); sRows = r.rows_total; sDigest = r.result_digest?.sha256 ?? null;
      }
      res.server_ms = sT;
      // 3. browser, only when the planner offers it (no override of its safety envelope)
      const bT: number[] = [];
      let bRows: number | null = null, bDigest: string | null = null;
      if (brw?.feasible && !c.mustNotUseBrowser) {
        for (let i = 0; i < 2; i++) {
          const t0 = performance.now();
          const placed = await deps.post<RunResp>("/api/query", { ...c.req, engine: "duckdb-wasm", client: { warm: i > 0 } });
          if (placed.placement !== "browser") { res.note = "the planner declined the browser on the explicit request"; break; }
          const r = await deps.resolve(placed);
          bT.push(performance.now() - t0); bRows = r.rows_total; bDigest = r.result_digest?.sha256 ?? null;
          res.browser_phases = r.phases ?? null;
        }
        res.browser_ms = bT;
        res.digests_match = bT.length && sDigest && bDigest ? sDigest === bDigest : null;
      } else {
        res.status = "skipped";
        res.note = res.note ?? (c.mustNotUseBrowser ? "browser must not be used for this case" : "the planner does not offer the browser here");
      }
      res.rows = { server: sRows, browser: bRows };
    } catch (e) {
      res.status = "error";
      res.note = e instanceof Error ? e.message : String(e);
    }
    report.results.push(res);
    await deps.report(report);
  }
  report.done = true;
  await deps.report(report);
  return report;
}
