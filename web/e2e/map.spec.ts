import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { openApp } from "./helpers.ts";

// Release gate: the map shows the REAL result geometries, one polygon per feature, never the dataset extent or a row-group rectangle.
// These tests count features and look at what is actually rendered; "a source exists" is not enough.

type GF = {
  map: { queryRenderedFeatures: (o: unknown) => { properties: Record<string, unknown>; geometry: GeoJSON.Geometry }[]; project: (c: [number, number]) => { x: number; y: number }; getCanvas: () => HTMLCanvasElement; getLayoutProperty: (id: string, p: string) => unknown; getFilter: (id: string) => unknown; areTilesLoaded: () => boolean };
  state: { limit: number; busy: boolean };
  run: () => Promise<void>;
  resultInfo: () => Promise<{ features: number; distinct: number; extent: number[] | null; maxFeatureShareOfDataset: number }>;
  layerVisibility: (id: string) => string | null;
  layerOrder: () => string[];
  setMode: (m: string) => void;
  applyMapVisibility: () => void;
};

/** Wait until the map has drawn everything it is going to draw (returns at once if it already has). */
async function settled(page: Page) {
  await page.evaluate(() => new Promise<void>((done) => {
    const m = (window as unknown as { __gf: { map: { loaded: () => boolean; isMoving: () => boolean; once: (e: string, f: () => void) => void; triggerRepaint: () => void } } }).__gf.map;
    const check = () => { if (m.loaded() && !m.isMoving()) done(); else m.once("idle", () => done()); };
    m.triggerRepaint();
    setTimeout(check, 150);
  }));
}

async function runWithLimit(page: Page, limit: number) {
  await page.evaluate(async (n) => { const g = (window as unknown as { __gf: GF }).__gf; g.state.limit = n; await g.run(); }, limit);
  await expect(page.getByTestId("status")).toContainText(/results|kết quả/);
  await settled(page);
}
const info = (page: Page) => page.evaluate(() => (window as unknown as { __gf: GF }).__gf.resultInfo());
const rendered = (page: Page, layer: string) => page.evaluate((l) => (window as unknown as { __gf: GF }).__gf.map.queryRenderedFeatures({ layers: [l] }).length, layer);

test.describe("result geometry on the map (Explorer, basemap tiles blocked)", () => {
  test.beforeEach(async ({ page }) => { await openApp(page, { mode: "explorer", lang: "vi" }); });

  test("the first view shows many separate building polygons, not a bounding rectangle", async ({ page }) => {
    const i = await info(page);
    expect(i.features, "the result holds many features").toBeGreaterThan(100);
    expect(i.distinct, "every feature has its own geometry").toBe(i.features);
    expect(i.maxFeatureShareOfDataset, "no feature covers a visible share of the dataset extent (that would be a bbox/extent rectangle)").toBeLessThan(0.001);
    await settled(page);
    expect(await rendered(page, "res-fill"), "individual polygons are actually drawn").toBeGreaterThan(20);
  });

  test("LIMIT 10 renders exactly 10 distinct features", async ({ page }) => {
    await runWithLimit(page, 10);
    const i = await info(page);
    expect(i.features).toBe(10);
    expect(i.distinct).toBe(10);
    expect(i.maxFeatureShareOfDataset).toBeLessThan(0.001);
    const n = await rendered(page, "res-fill");
    expect(n, "the ten polygons are in view and drawn").toBeGreaterThanOrEqual(5);
    expect(n).toBeLessThanOrEqual(10);
  });

  test("LIMIT 100 renders 100 distinct features", async ({ page }) => {
    await runWithLimit(page, 100);
    const i = await info(page);
    expect(i.features).toBe(100);
    expect(i.distinct).toBe(100);
    expect(await rendered(page, "res-fill")).toBeGreaterThan(30);
  });

  test("debug overlays are off in Explorer; the result layers are above them", async ({ page }) => {
    for (const id of ["rg-fill", "rg-line", "extent-line"]) expect(await page.evaluate((l) => (window as unknown as { __gf: GF }).__gf.layerVisibility(l), id), id).toBe("none");
    const order = await page.evaluate(() => (window as unknown as { __gf: GF }).__gf.layerOrder());
    const at = (id: string) => order.indexOf(id);
    expect(at("rg-fill")).toBeLessThan(at("res-fill"));
    expect(at("extent-line")).toBeLessThan(at("res-fill"));
    expect(at("res-fill")).toBeLessThan(at("res-sel-fill"));
    expect(at("res-sel-fill")).toBeLessThan(at("res-sel"));
    expect(await rendered(page, "rg-fill")).toBe(0);
  });

  test("clicking a polygon selects exactly that feature and opens the inspector", async ({ page }) => {
    await expect(page.getByTestId("inspector-props")).toHaveCount(0);
    const target = await page.evaluate(() => {
      const g = (window as unknown as { __gf: GF }).__gf;
      const fs = g.map.queryRenderedFeatures({ layers: ["res-fill"] });
      const f = fs[Math.floor(fs.length / 2)];
      const ring = (f.geometry as GeoJSON.Polygon).coordinates[0];
      const cx = ring.reduce((a, p) => a + p[0], 0) / ring.length, cy = ring.reduce((a, p) => a + p[1], 0) / ring.length;
      const px = g.map.project([cx, cy]);
      const r = g.map.getCanvas().getBoundingClientRect();
      // synthetic buildings overlap: the polygon under the cursor is the top-most one at that pixel
      const top = (g.map as unknown as { queryRenderedFeatures: (p: [number, number], o: unknown) => { properties: Record<string, unknown> }[] }).queryRenderedFeatures([px.x, px.y], { layers: ["res-fill"] })[0];
      return { x: r.left + px.x, y: r.top + px.y, i: top.properties.__i };
    });
    await page.mouse.click(target.x, target.y);
    await expect(page.getByTestId("inspector-props")).toBeVisible();
    await settled(page);
    const sel = await page.evaluate(() => { const g = (window as unknown as { __gf: GF }).__gf; return { filter: g.map.getFilter("res-sel-fill"), n: g.map.queryRenderedFeatures({ layers: ["res-sel-fill"] }).length }; });
    expect(JSON.stringify(sel.filter)).toContain(String(target.i));
    expect(sel.n, "one highlighted geometry").toBe(1);
  });
});

test("Developer mode: the debug layers are optional, off by default, and never cover the result", async ({ page }) => {
  await openApp(page, { mode: "developer", lang: "en" });
  const vis = (id: string) => page.evaluate((l) => (window as unknown as { __gf: GF }).__gf.layerVisibility(l), id);
  expect(await vis("rg-fill")).toBe("none");
  await page.getByTestId("settings").click();
  await page.getByTestId("overlays-toggle").check();
  expect(await vis("rg-fill")).toBe("visible");
  expect(await vis("extent-line")).toBe("visible");
  await page.waitForTimeout(300);
  expect(await rendered(page, "res-fill"), "the result is still drawn with the overlays on").toBeGreaterThan(20);
  await page.getByTestId("overlays-toggle").uncheck();
  expect(await vis("rg-fill")).toBe("none");
  // switching to Explorer hides them again whatever the toggle says
  await page.getByTestId("overlays-toggle").check();
  await page.evaluate(() => (window as unknown as { __gf: GF }).__gf.setMode("explorer"));
  expect(await vis("rg-fill")).toBe("none");
});

test("basemap: road/water layers sit under the result, place names above it, and the basemap can be switched off", async ({ page }) => {
  // a tiny style stands in for the public map server, so the test does not depend on the network
  const world = { type: "FeatureCollection", features: [
    { type: "Feature", properties: { name: "Sông" }, geometry: { type: "Polygon", coordinates: [[[90, 0], [120, 0], [120, 30], [90, 30], [90, 0]]] } },
    { type: "Feature", properties: { name: "Đường" }, geometry: { type: "LineString", coordinates: [[90, 10], [120, 20]] } },
    { type: "Feature", properties: { name: "Đắk Lắk" }, geometry: { type: "Point", coordinates: [108, 12.7] } },
  ] };
  const style = { version: 8, glyphs: "https://tiles.openfreemap.org/fonts/{fontstack}/{range}.pbf", sources: { w: { type: "geojson", data: world } }, layers: [
    { id: "background", type: "background", paint: { "background-color": "#101820" } },
    { id: "water", type: "fill", source: "w", filter: ["==", ["geometry-type"], "Polygon"], paint: { "fill-color": "#1b3a57" } },
    { id: "road", type: "line", source: "w", filter: ["==", ["geometry-type"], "LineString"], paint: { "line-color": "#556" } },
    { id: "label_place", type: "symbol", source: "w", filter: ["==", ["geometry-type"], "Point"], layout: { "text-field": ["get", "name"], "text-font": ["Noto Sans Regular"] } },
  ] };
  await page.route(/tiles\.openfreemap\.org\/fonts/, (r) => r.abort());
  await page.route(/tiles\.openfreemap\.org\/styles\/dark/, (r) => r.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(style), headers: { "access-control-allow-origin": "*" } }));
  await openApp(page, { mode: "explorer", lang: "vi", tiles: "network" });
  await settled(page);
  const g = (fn: string) => page.evaluate((f) => { const x = (window as unknown as { __gf: Record<string, () => unknown> }).__gf; return x[f](); }, fn);
  expect(await g("basemapLoaded"), "the basemap style was installed").toBe(true);
  const order = (await g("layerOrder")) as string[];
  const at = (id: string) => order.indexOf(id);
  expect(at("water")).toBeGreaterThan(-1);
  expect(at("water")).toBeLessThan(at("rg-fill"));
  expect(at("road")).toBeLessThan(at("res-fill"));
  expect(at("res-sel")).toBeLessThan(at("label_place"));
  const vis = (id: string) => page.evaluate((l) => (window as unknown as { __gf: GF }).__gf.layerVisibility(l), id);
  expect(await vis("water")).toBe("visible");
  expect(await rendered(page, "water"), "the water polygon is actually drawn").toBeGreaterThan(0);
  await page.getByTestId("settings").click();
  await page.getByTestId("basemap-toggle").uncheck();
  expect(await vis("water")).toBe("none");
  expect(await vis("label_place")).toBe("none");
  expect(await vis("res-fill"), "the result stays").toBe("visible");
});

test("basemap server unreachable: the map still works on a plain background", async ({ page }) => {
  await openApp(page, { mode: "explorer", lang: "vi" });
  await settled(page);
  expect(await page.evaluate(() => (window as unknown as { __gf: GF & { basemapLoaded: () => boolean } }).__gf.basemapLoaded())).toBe(false);
  expect((await info(page)).features).toBeGreaterThan(100);
});

test("real data: the Overture building subset draws as separate footprints (default dataset when present)", async ({ page }) => {
  await openApp(page, { mode: "explorer", lang: "vi", source: "overture_buildings_vn_south" });
  const i = await info(page);
  test.skip(i.features === 0, "Overture subset not present");
  expect(i.features).toBeGreaterThan(100);
  expect(i.distinct).toBe(i.features);
  expect(i.maxFeatureShareOfDataset).toBeLessThan(0.001);
  await settled(page);
  expect(await rendered(page, "res-fill")).toBeGreaterThan(20);
});
