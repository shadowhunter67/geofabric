import { expect, test } from "@playwright/test";
import * as fs from "node:fs";
import { openApp } from "./helpers.ts";

// Measurements on the real data pack (PACK_PERF=1): three viewports, planner choice, execution time, rows, render time, heap.
// Evidence for the report: written to benchmarks/results/pack-performance.json. Not a pass/fail benchmark of speed: it asserts
// that every viewport answers, stays under the render guard and never crashes the tab.

type G = {
  map: { jumpTo: (o: unknown) => void; once: (e: string, f: () => void) => void; loaded: () => boolean; isMoving: () => boolean; queryRenderedFeatures: (o: unknown) => unknown[] };
  state: { last: { rows_total: number; placement?: string; physical: { engine: string }; timing: { total_ms: { value: number | null } }; geojson: { features: unknown[] } | null } | null };
  run: () => Promise<void>;
};

const VIEWPORTS = [
  { name: "small", center: [106.6985, 10.7765] as [number, number], zoom: 16 },
  { name: "medium", center: [106.70, 10.78] as [number, number], zoom: 14 },
  { name: "large", center: [106.70, 10.78] as [number, number], zoom: 12 },
];
const SOURCES: { id: string; extra: Record<string, unknown> }[] = [
  { id: "overture_transport_hcm", extra: {} },
  { id: "overture_places_hcm", extra: {} },
  { id: "overture_admin_hcm", extra: {} },
  { id: "overture_landuse_hcm", extra: {} },
];

test("pack performance: three viewports over the real data", async ({ page }) => {
  test.skip(!process.env.PACK_PERF, "set PACK_PERF=1");
  test.setTimeout(600_000);
  await openApp(page, { mode: "developer", lang: "en", source: "overture_buildings_vn_south" });
  const rows: Record<string, unknown>[] = [];
  for (const v of VIEWPORTS) {
    // active dataset (buildings) through the UI path: plan -> placement -> result -> draw
    const m = await page.evaluate(async ([center, zoom]) => {
      const g = (window as unknown as { __gf: G }).__gf;
      const heap = () => (performance as unknown as { memory?: { usedJSHeapSize: number } }).memory?.usedJSHeapSize ?? 0;
      const h0 = heap();
      g.map.jumpTo({ center, zoom });
      const t0 = performance.now();
      await g.run();
      const tResponse = performance.now() - t0;
      await new Promise<void>((done) => { const t1 = Date.now(); const poll = () => (g.map.loaded() && !g.map.isMoving()) || Date.now() - t1 > 30000 ? done() : setTimeout(poll, 50); setTimeout(poll, 50); });
      const tDrawn = performance.now() - t0;
      const last = g.state.last!;
      return { rows: last.rows_total, features: last.geojson?.features.length ?? 0, engine: last.physical.engine, placement: last.placement ?? "server",
        engine_ms: last.timing.total_ms.value, request_to_response_ms: Math.round(tResponse), request_to_drawn_ms: Math.round(tDrawn), heap_mb_delta: Math.round((heap() - h0) / 1e6) };
    }, [v.center, v.zoom] as const);
    rows.push({ dataset: "overture_buildings_vn_south", viewport: v.name, zoom: v.zoom, ...m });
    expect(m.features).toBeGreaterThan(0);
    expect(m.features, "render guard: never more than the 20,000 default limit").toBeLessThanOrEqual(20000);

    // the other datasets through the same API (server planner), viewport of the map
    for (const s of SOURCES) {
      const r = await page.evaluate(async ([id, center, zoom]) => {
        const g = (window as unknown as { __gf: G }).__gf;
        g.map.jumpTo({ center, zoom });
        const half = 360 / (512 * 2 ** (zoom as number)) * 800; // ~ viewport half-width in degrees at this zoom (800 px)
        const c = center as [number, number];
        const bbox = [c[0] - half, c[1] - half * 0.6, c[0] + half, c[1] + half * 0.6];
        const t0 = performance.now();
        const res = await fetch("/api/query", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ source: id, kind: "features", bbox, crs: "OGC:CRS84", limit: 20000, browser: true }) });
        const j = await res.json();
        return { status: res.status, ms: Math.round(performance.now() - t0), rows: j.rows_total ?? null, placement: j.placement ?? j.physical?.placement ?? null, engine: j.physical?.engine ?? null, bytes: JSON.stringify(j.geojson ?? "").length };
      }, [s.id, v.center, v.zoom] as const);
      rows.push({ dataset: s.id, viewport: v.name, zoom: v.zoom, ...r });
      expect(r.status).toBe(200);
    }
  }
  fs.writeFileSync("../benchmarks/results/pack-performance.json", JSON.stringify({ measured: new Date().toISOString(), machine: "one Windows machine (dev box), Chrome 155, gateway debug build + prebuilt DuckDB", rows }, null, 1));
  console.log(JSON.stringify(rows));
});
