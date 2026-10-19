import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { openApp, post } from "./helpers.ts";

// Representative rendering (ADR-012): when a viewport holds more features than the map draws, the drawn sample must look like the
// viewport, not like the first rows of the file. Real data (the Overture buildings), measured against the FULL logical counts.

type GF = {
  map: { jumpTo: (o: unknown) => void; getBounds: () => { getWest(): number; getEast(): number; getSouth(): number; getNorth(): number }; getZoom: () => number };
  state: {
    representative: boolean; limit: number; sampling: { total: number; shown: number; modulus: number; budget: number } | null;
    last: { rows_total: number; result_digest?: { sha256: string }; geojson: { features: { geometry: { coordinates: number[][][] } }[] } | null } | null;
  };
  run: () => Promise<void>;
  resolvePlacement: (r: unknown) => Promise<{ rows_total: number; result_digest?: { sha256: string } }>;
};

async function viewportRun(page: Page, representative: boolean) {
  return page.evaluate(async (rep) => {
    const g = (window as unknown as { __gf: GF }).__gf;
    g.state.representative = rep;
    g.state.limit = 20000;
    g.map.jumpTo({ center: [106.69, 10.79], zoom: 12.6 });
    await g.run();
    const b = g.map.getBounds();
    const feats = g.state.last?.geojson?.features ?? [];
    const cx = (b.getWest() + b.getEast()) / 2, cy = (b.getSouth() + b.getNorth()) / 2;
    const q = [0, 0, 0, 0];
    for (const f of feats) {
      const r = f.geometry.coordinates[0][0];
      q[(r[0] > cx ? 1 : 0) + (r[1] > cy ? 2 : 0)]++;
    }
    return { bbox: [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()], cx, cy, quadrants: q, shown: feats.length, sampling: g.state.sampling, digest: g.state.last?.result_digest?.sha256 ?? null };
  }, representative);
}

test.describe("representative rendering (real buildings, basemap blocked)", () => {
  test.beforeEach(async ({ page }) => { await openApp(page, { mode: "explorer", lang: "vi", source: "overture_buildings_vn_south" }); });

  test("the sample follows the true distribution of the viewport; the old file-order cut does not", async ({ page }) => {
    const sampled = await viewportRun(page, true);
    test.skip(!sampled.sampling, "the viewport holds fewer features than the budget (data pack not present?)");
    expect(sampled.sampling!.total).toBeGreaterThan(20000);
    expect(sampled.shown).toBeLessThanOrEqual(20000);
    expect(sampled.shown).toBeGreaterThanOrEqual(sampled.sampling!.budget / 2 - 100);
    expect(sampled.sampling!.shown).toBe(sampled.shown);
    expect(Number.isInteger(Math.log2(sampled.sampling!.modulus))).toBe(true);

    // truth: the FULL logical count of each quadrant of the same viewport (counts are never sampled)
    const [x0, y0, x1, y1] = sampled.bbox, { cx, cy } = sampled;
    const boxes = [[x0, y0, cx, cy], [cx, y0, x1, cy], [x0, cy, cx, y1], [cx, cy, x1, y1]];
    const truth: number[] = [];
    for (const b of boxes) {
      const r = await post(page, "/api/query", { source: "overture_buildings_vn_south", kind: "count", bbox: b, crs: "OGC:CRS84", browser: false });
      expect(r.status).toBe(200);
      truth.push(Number(r.json.rows[0].count));
    }
    // a feature belongs to the quadrant of its first vertex; bbox-intersecting features straddling a split line shift a little
    const sumT = truth.reduce((a, b) => a + b, 0), sumS = sampled.quadrants.reduce((a, b) => a + b, 0);
    const dev = (q: number[], t: number[], sq: number, st: number) => Math.max(...q.map((v, i) => Math.abs(v / sq - t[i] / st)));
    const sampleDeviation = dev(sampled.quadrants, truth, sumS, sumT);
    expect(sampleDeviation, `sample quadrant shares ${sampled.quadrants.map((v) => (v / sumS).toFixed(3))} vs truth ${truth.map((v) => (v / sumT).toFixed(3))}`).toBeLessThan(0.04);

    // the old behaviour for contrast: first 20,000 rows in file order
    const cut = await viewportRun(page, false);
    const sumC = cut.quadrants.reduce((a, b) => a + b, 0);
    const cutDeviation = dev(cut.quadrants, truth, sumC, sumT);
    expect(cut.sampling).toBeNull();
    expect(cutDeviation, `cut quadrant shares ${cut.quadrants.map((v) => (v / sumC).toFixed(3))}`).toBeGreaterThan(sampleDeviation * 2);
  });

  test("the sample is deterministic, the count is the full logical count, and the user is told", async ({ page }) => {
    const a = await viewportRun(page, true);
    test.skip(!a.sampling, "the viewport holds fewer features than the budget");
    const b = await viewportRun(page, true);
    expect(b.digest).toBe(a.digest);
    expect(b.sampling).toEqual(a.sampling);
    await expect(page.getByTestId("sample-hint")).toContainText("mẫu đại diện");
    await expect(page.getByTestId("sample-hint")).toContainText(a.sampling!.shown.toLocaleString("vi-VN"));
    await expect(page.getByTestId("sample-hint")).toContainText(a.sampling!.total.toLocaleString("vi-VN"));
    const full = await post(page, "/api/query", { source: "overture_buildings_vn_south", kind: "count", bbox: a.bbox, crs: "OGC:CRS84", browser: false });
    expect(Number(full.json.rows[0].count)).toBe(a.sampling!.total);
    // an aggregate refuses to be sampled: it can never see a thinned input
    const bad = await post(page, "/api/query", { source: "overture_buildings_vn_south", kind: "count", bbox: a.bbox, crs: "OGC:CRS84", sample_modulus: 4 });
    expect(bad.status).toBe(400);
  });

  test("the same sample is drawn whether the server or the browser engine executes it (md5 is portable)", async ({ page }) => {
    const req = { source: "clustered_10mb_spatial", kind: "features", bbox: [100.5, 8.5, 109.5, 23.5], crs: "EPSG:4326", limit: 100000, sample_modulus: 8 };
    const server = await post(page, "/api/query", { ...req, engine: "duckdb", browser: false });
    expect(server.status).toBe(200);
    const planned = await post(page, "/api/query", { ...req, engine: "duckdb-wasm", browser: true });
    test.skip(planned.json.placement !== "browser", "the planner did not offer the browser for this file");
    const browser = await page.evaluate((r) => (window as unknown as { __gf: GF }).__gf.resolvePlacement(r), planned.json);
    expect(browser.rows_total).toBe(server.json.rows_total);
    expect(browser.result_digest?.sha256).toBe(server.json.result_digest.sha256);
  });
});
