import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// Planner hardening (ADR-009/010) in a real browser: runtime probe, health, fallback taxonomy, UI fields.
// Needs: `npm run build`, benchmarks/data (10mb files). The gateway is shared between tests: every test
// resets the planner state first.

interface Probe { engine: string; session: string; engine_version: string; checks: { name: string; ok: boolean; required: boolean; detail: string }[]; supported_ops: string[]; memory_class: string }
interface GF {
  probeRuntime(reason: string): Promise<void>;
  placeWithFallback(first: unknown, retry: () => Promise<unknown>): Promise<{ fallback?: { class: string; from: string } }>;
  state: { probe: Probe | null };
  wasm: { execute: (...a: unknown[]) => Promise<unknown>; recycles: number };
}
declare global {
  interface Window { __gf: GF }
}

const BBOX = [104.0, 12.0, 105.0, 13.0];
const q = (extra: Record<string, unknown> = {}) => ({ source: "clustered_10mb_spatial", kind: "features", bbox: BBOX, crs: "EPSG:4326", limit: 2000, ...extra });

async function post(page: Page, path: string, body: unknown = {}) {
  return page.evaluate(async ([p, b]) => {
    const r = await fetch(p as string, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(b) });
    return { status: r.status, json: await r.json() };
  }, [path, body] as const);
}
async function get(page: Page, path: string) {
  return page.evaluate(async (p) => (await fetch(p)).json(), path);
}

async function open(page: Page) {
  await page.goto("/?e2e=1");
  await expect(page.locator("header .status")).toContainText("rows ·", { timeout: 30_000 });
  // the startup probe finishes shortly after the map loaded
  await page.waitForFunction(() => window.__gf.state.probe !== null, null, { timeout: 60_000 });
}

test.beforeEach(async ({ page }) => {
  await open(page);
  await post(page, "/api/state/reset"); // forget probes/health of earlier tests (shared gateway)
  await page.evaluate(() => window.__gf.probeRuntime("test"));
});

test("runtime probe: runs in the real browser, passes the required checks and is stored by the planner", async ({ page }) => {
  const probe = await page.evaluate(() => window.__gf.state.probe!);
  console.log("PROBE " + JSON.stringify(probe.checks.map((c) => `${c.name}=${c.ok}${c.ok ? "" : " (" + c.detail.slice(0, 80) + ")"}`)));
  for (const c of probe.checks.filter((c) => c.required)) expect(c.ok, `${c.name}: ${c.detail}`).toBe(true);
  expect(probe.supported_ops).toEqual(expect.arrayContaining(["source", "filter", "bbox_filter", "aggregate", "project", "limit", "sort"]));
  expect(probe.checks.find((c) => c.name === "read_class:plain_wkb")?.ok).toBe(true);
  const st = await get(page, "/api/state");
  expect(st.probes["duckdb-wasm"].session).toBe(probe.session);
  expect(st.probes["duckdb-wasm"].engine_version).toBe(probe.engine_version);
});

test("probe result drives pre-rejection: a file class this runtime cannot read is never sent to it", async ({ page }) => {
  const probe = await page.evaluate(() => window.__gf.state.probe!);
  const typedOk = probe.checks.find((c) => c.name === "read_class:duckdb_typed")!.ok;
  const plan = await post(page, "/api/query/plan", { source: "overture_buildings_vn_south", kind: "features", bbox: [106.6, 10.7, 106.7, 10.8], crs: "OGC:CRS84", limit: 2000 });
  if (plan.status === 404) test.skip(true, "Overture subset not present");
  const wasm = (plan.json.physical.candidates as { engine: string; feasible: boolean; rejected_because: string[] }[]).find((c) => c.engine === "duckdb-wasm")!;
  expect(wasm.feasible).toBe(false); // too large for the tab either way (memory risk) ...
  if (!typedOk) expect(wasm.rejected_because.join(" ")).toContain("duckdb_typed"); // ... and the probe says why it could not read it anyway
  console.log("PROBE duckdb_typed readable in this runtime: " + typedOk);
});

test("Execution Plan shows engine, confidence, health, runtime probe, network profile, risk and fallback", async ({ page }) => {
  await page.getByRole("tab", { name: "SQL / Ask" }).click();
  await page.getByLabel("Engine").selectOption("duckdb-wasm");
  await page.getByRole("button", { name: "Run viewport query" }).click();
  await expect(page.locator("header .status")).toContainText("duckdb-wasm", { timeout: 30_000 });
  await page.getByRole("tab", { name: "Execution Plan" }).click();
  const a = page.getByTestId("assessment");
  await expect(a).toContainText("Engine");
  await expect(a.getByTestId("confidence-badge")).toContainText(/Confidence|Medium|High/);
  await expect(a.getByTestId("confidence-factors")).toBeVisible();
  await expect(a.getByTestId("health-duckdb-wasm")).toContainText(/Unknown|Healthy/); // health is what was known when the plan was made
  await expect.poll(async () => (await get(page, "/api/state")).health["duckdb-wasm"].state, { timeout: 10_000 }).toBe("healthy"); // the run itself was reported
  await expect(a.getByTestId("probe-badge")).toContainText("Observed");
  await expect(a).toContainText("Network profile");
  await expect(a).toContainText("LOCAL");
  await expect(a.getByTestId("risk")).toContainText("crash threshold");
  await expect(a).toContainText("Fallback");
  await expect(a).toContainText("Estimated");
  await expect(a.getByTestId("estimate-vs-actual")).toContainText("rows");
});

test("the assumed network profile changes the browser's cost (latency of range requests)", async ({ page }) => {
  const cost = async (net: string) => {
    const p = await post(page, "/api/query/plan", q({ network: net }));
    const b = p.json.physical.candidates.find((c: { engine: string }) => c.engine === "duckdb-wasm");
    return { latency: b.cost.latency_ms as number, score: b.score_ms as number, net: p.json.physical.assessment.network.name as string };
  };
  const local = await cost("LOCAL"), mobile = await cost("MOBILE"), slow = await cost("HIGH_LATENCY");
  expect([local.net, mobile.net, slow.net]).toEqual(["LOCAL", "MOBILE", "HIGH_LATENCY"]);
  expect(local.latency).toBe(0);
  expect(slow.latency).toBeGreaterThan(mobile.latency);
  expect(mobile.score).toBeGreaterThan(local.score + 500);
});

test("a crash report makes the browser unhealthy, invalidates its probe and the planner stops offering it", async ({ page }) => {
  const rep = await post(page, "/api/report", { engine: "duckdb-wasm", source: "clustered_10mb_spatial", ok: false, message: "Target crashed" });
  expect(rep.json.class).toBe("ENGINE_CRASH");
  const st = await get(page, "/api/state");
  expect(st.health["duckdb-wasm"].state).toBe("unhealthy");
  expect(st.probes["duckdb-wasm"]).toBeUndefined();
  const plan = await post(page, "/api/query/plan", q());
  const b = plan.json.physical.candidates.find((c: { engine: string }) => c.engine === "duckdb-wasm");
  expect(b.feasible).toBe(false);
  expect(b.rejected_because.join(" ")).toContain("unhealthy");
  expect(plan.json.physical.engine).toBe("duckdb");
  // Healthy runs recover it after the cool-down in real use; here a reset stands in for the passage of time
});

test("a deterministic failure on a source is remembered and not repeated; health is untouched", async ({ page }) => {
  await post(page, "/api/report", { engine: "duckdb-wasm", source: "clustered_10mb_spatial", ok: false, message: "TransactionContext::ActiveTransaction called without active transaction" });
  const st = await get(page, "/api/state");
  expect(st.incompat).toHaveLength(1);
  expect(st.incompat[0]).toMatchObject({ engine: "duckdb-wasm", class: "UNSUPPORTED_FORMAT" });
  expect(st.health["duckdb-wasm"].state).not.toBe("unhealthy");
  const plan = await post(page, "/api/query/plan", q());
  const b = plan.json.physical.candidates.find((c: { engine: string }) => c.engine === "duckdb-wasm");
  expect(b.rejected_because.join(" ")).toContain("failed before");
  // another source is unaffected
  const other = await post(page, "/api/query/plan", q({ source: "uniform_10mb_spatial" }));
  if (other.status === 200) expect(other.json.physical.candidates.find((c: { engine: string }) => c.engine === "duckdb-wasm").rejected_because.join(" ")).not.toContain("failed before");
});

test("fallback is bounded: one fallback to the server, classified, none on cancellation, never a retry", async ({ page }) => {
  const planned = await post(page, "/api/query", q({ engine: "duckdb-wasm" }));
  expect(planned.json.placement).toBe("browser");
  const run = (err: string) =>
    page.evaluate(async ([first, msg]) => {
      let retries = 0;
      window.__gf.wasm.execute = () => Promise.reject(new Error(msg as string));
      try {
        const r = await window.__gf.placeWithFallback(first, async () => { retries++; return { placement: "server", physical: (first as { physical: unknown }).physical, rows_total: 0 }; });
        return { ok: true, cls: r.fallback?.class ?? null, retries };
      } catch (e) {
        return { ok: false, error: String(e), retries };
      }
    }, [planned.json, err] as const);

  const crash = await run("Target crashed");
  expect(crash).toMatchObject({ ok: true, cls: "ENGINE_CRASH", retries: 1 });
  const fmt = await run("TransactionContext::ActiveTransaction called without active transaction");
  expect(fmt).toMatchObject({ ok: true, cls: "UNSUPPORTED_FORMAT", retries: 1 });
  const cancelled = await run("browser query cancelled");
  expect(cancelled).toMatchObject({ ok: false, retries: 0 }); // the user's decision: no fallback
  // the crash invalidated and re-probed the runtime in the background: a probe exists again shortly after
  await page.waitForFunction(() => window.__gf.state.probe !== null, null, { timeout: 60_000 });
});
