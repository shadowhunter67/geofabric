import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { openApp } from "./helpers.ts";

// The Vietnam data pack in the product UI: catalog, layers, type-specific inspector, demo workflows. Uses the real pack in
// data/local (skipped when it is not there); the basemap is blocked, the data layers do not depend on it.

type GF = {
  map: {
    jumpTo: (o: unknown) => void; getZoom: () => number; project: (c: [number, number]) => { x: number; y: number };
    queryRenderedFeatures: (a: unknown, b?: unknown) => { properties: Record<string, unknown>; geometry: GeoJSON.Geometry }[];
    getPaintProperty: (id: string, p: string) => unknown; getStyle: () => { layers: { id: string }[] }; getCanvas: () => HTMLCanvasElement; loaded: () => boolean; isMoving: () => boolean;
  };
  state: { logs: string[]; busy: boolean; raster: Map<string, { overlay: { min: number; max: number } | null }> };
  contextInfo: () => Record<string, { features: number; loading: boolean; limitHit: boolean }>;
};
const G = "(window as unknown as { __gf: any }).__gf";
void G;

async function packPresent(page: Page): Promise<boolean> {
  return page.evaluate(async () => {
    const r = await fetch("/api/catalog");
    if (!r.ok) return false;
    const c = (await r.json()) as { datasets: { id: string; present: boolean }[] };
    return ["overture_buildings_vn_south", "overture_transport_hcm", "overture_places_hcm", "overture_admin_hcm"].every((id) => c.datasets.some((d) => d.id === id && d.present));
  });
}

async function goTo(page: Page, center: [number, number], zoom: number) {
  await page.evaluate(([c, z]) => (window as unknown as { __gf: GF }).__gf.map.jumpTo({ center: c, zoom: z }), [center, zoom] as const);
}
async function settled(page: Page) {
  await page.evaluate(() => new Promise<void>((done) => {
    const m = (window as unknown as { __gf: GF }).__gf.map;
    const t0 = Date.now();
    const poll = () => (m.loaded() && !m.isMoving()) || Date.now() - t0 > 15000 ? done() : setTimeout(poll, 150);
    setTimeout(poll, 150);
  }));
}
const layerCount = (page: Page, prefix: string) => page.evaluate((p) => {
  const m = (window as unknown as { __gf: GF }).__gf.map;
  const ids = m.getStyle().layers.map((l) => l.id).filter((id) => id.startsWith(p));
  return ids.length ? m.queryRenderedFeatures({ layers: ids }).length : 0;
}, prefix);
const toggle = (page: Page, id: string) => page.locator(`[data-testid="dataset"][data-id="${id}"] input[type="checkbox"]`);

test.describe("data pack (Explorer, Vietnamese)", () => {
  test.beforeEach(async ({ page }) => {
    await openApp(page, { mode: "explorer", lang: "vi", source: "overture_buildings_vn_south" });
    test.skip(!(await packPresent(page)), "the Vietnam data pack is not in data/local (run: python data/scripts/geo_data.py bootstrap \"Ho Chi Minh City\" --run)");
  });

  test("catalog groups with friendly names; no file names or raw ids in Explorer", async ({ page }) => {
    const names = await page.getByTestId("dataset").locator(".name").allInnerTexts();
    for (const n of ["Tòa nhà", "Đường giao thông", "Ranh giới hành chính", "Địa điểm", "Sử dụng đất", "Độ cao địa hình"]) expect(names).toContain(n);
    const heads = await page.locator("aside.left .catalog-group").allInnerTexts();
    expect(heads.join("|").toLowerCase()).toContain("giao thông");
    const text = await page.locator("aside.left").innerText();
    expect(text).not.toMatch(/overture_|\.parquet|clustered_|uniform_/);
  });

  test("roads, places and administrative units draw as separate layers over the real data", async ({ page }) => {
    for (const id of ["overture_transport_hcm", "overture_places_hcm", "overture_admin_hcm", "overture_landuse_hcm"]) await toggle(page, id).check();
    await goTo(page, [106.6985, 10.7765], 15);
    await expect.poll(async () => (await page.evaluate(() => (window as unknown as { __gf: GF }).__gf.contextInfo()))["overture_places_hcm"]?.features ?? 0, { timeout: 60_000 }).toBeGreaterThan(50);
    await expect.poll(async () => (await page.evaluate(() => (window as unknown as { __gf: GF }).__gf.contextInfo()))["overture_transport_hcm"]?.features ?? 0, { timeout: 60_000 }).toBeGreaterThan(50);
    await expect.poll(async () => (await page.evaluate(() => (window as unknown as { __gf: GF }).__gf.contextInfo()))["overture_admin_hcm"]?.features ?? 0, { timeout: 60_000 }).toBeGreaterThan(0);
    await settled(page);
    // tiles of a new source are cut a moment after the source is added: poll instead of reading once
    await expect.poll(() => layerCount(page, "ctx-overture_transport_hcm-"), { timeout: 20_000 }).toBeGreaterThan(20);
    await expect.poll(() => layerCount(page, "ctx-overture_places_hcm-"), { timeout: 20_000 }).toBeGreaterThan(20);
    await expect.poll(() => layerCount(page, "ctx-overture_admin_hcm-"), { timeout: 20_000 }).toBeGreaterThan(0);
    // the data credit mentions OpenStreetMap while catalog layers are visible
    await expect(page.locator(".map-attribution")).toContainText("OpenStreetMap");
  });

  test("below its minimum zoom a layer says so instead of drawing a cut-off sample", async ({ page }) => {
    await toggle(page, "overture_places_hcm").check();
    await goTo(page, [106.70, 10.78], 11);
    await expect(page.locator('[data-testid="dataset"][data-id="overture_places_hcm"] .layer-status')).toContainText("Phóng to để xem", { timeout: 20_000 });
  });

  test("clicking a road, a place and a ward shows type-specific labelled rows", async ({ page }) => {
    for (const id of ["overture_transport_hcm", "overture_places_hcm", "overture_admin_hcm"]) await toggle(page, id).check();
    await goTo(page, [106.6985, 10.7765], 15.5);
    for (const id of ["overture_places_hcm", "overture_transport_hcm", "overture_admin_hcm"]) {
      await expect.poll(async () => (await page.evaluate(() => (window as unknown as { __gf: GF }).__gf.contextInfo()))[id]?.features ?? 0, { timeout: 60_000 }).toBeGreaterThan(0);
    }
    // the layers load in a cascade and refresh a second time ~1 s later (each refresh removes and re-adds a layer): wait for a picture
    // that stays the same, or the click lands while a layer is being swapped (flake investigation 2026-10-19: 20 % of solo runs)
    let before = "";
    await expect.poll(async () => {
      const now = await page.evaluate(() => { const ci = (window as unknown as { __gf: GF }).__gf.contextInfo() as Record<string, { features: number; loading: boolean }>; return JSON.stringify(Object.entries(ci).map(([k, v]) => [k, v.features, v.loading])); });
      const stable = now === before && !now.includes("true");
      before = now;
      return stable;
    }, { timeout: 30_000, intervals: [1200] }).toBe(true);
    await settled(page);
    const clickOn = async (prefix: string) => {
      const find = (p: string) => page.evaluate((p) => {
        const g = (window as unknown as { __gf: GF }).__gf;
        const clickable = g.map.getStyle().layers.map((l) => l.id).filter((id) => ["res-fill", "res-pt"].includes(id) || (id.startsWith("ctx-") && !id.endsWith("-label")));
        const canvas = g.map.getCanvas().getBoundingClientRect();
        // the app selects the TOP-MOST feature within 5 px: probe a grid for a pixel where that feature belongs to the wanted layer
        for (let y = 80; y < canvas.height - 80; y += 17) for (let x = 80; x < canvas.width - 80; x += 23) {
          const hit = (g.map as unknown as { queryRenderedFeatures: (b: unknown, o: unknown) => { layer: { id: string } }[] })
            .queryRenderedFeatures([[x - 5, y - 5], [x + 5, y + 5]], { layers: clickable })[0];
          if (hit && hit.layer.id.startsWith(p)) return { x: canvas.left + x, y: canvas.top + y };
        }
        return null;
      }, p);
      let pt: { x: number; y: number } | null = null;
      await expect.poll(async () => { pt = await find(prefix); return pt !== null; }, { timeout: 20_000, message: `a ${prefix} feature under some pixel` }).toBe(true);
      pt = pt as unknown as { x: number; y: number };
      await page.mouse.click(pt.x, pt.y);
    };
    await clickOn("ctx-overture_places_hcm-");
    await expect(page.getByTestId("inspector-props")).toContainText("Danh mục");
    await expect(page.getByTestId("inspector-props")).toContainText("Nguồn");
    await clickOn("ctx-overture_transport_hcm-");
    await expect(page.getByTestId("inspector-props")).toContainText("Phân loại");
    await clickOn("ctx-overture_admin_hcm-");
    await expect(page.getByTestId("inspector-props")).toContainText("Cấp hành chính");
    const html = await page.getByTestId("inspector-props").innerText();
    expect(html).not.toMatch(/place\.category|transport\.class|admin\.subtype|landuse\.class/);
  });

  test("switching a layer off removes it from the map; the active dataset's switch hides its result", async ({ page }) => {
    await toggle(page, "overture_admin_hcm").check();
    await goTo(page, [106.70, 10.78], 12);
    await expect.poll(() => layerCount(page, "ctx-overture_admin_hcm-"), { timeout: 60_000 }).toBeGreaterThan(0);
    await toggle(page, "overture_admin_hcm").uncheck();
    await expect.poll(() => page.evaluate(() => (window as unknown as { __gf: GF }).__gf.map.getStyle().layers.some((l) => l.id.startsWith("ctx-overture_admin_hcm-")))).toBe(false);
    // the active dataset (buildings) draws through the result layers: its switch shows and hides them
    const resVisibility = () => page.evaluate(() => (window as unknown as { __gf: { map: { getLayoutProperty: (id: string, p: string) => unknown } } }).__gf.map.getLayoutProperty("res-fill", "visibility") ?? "visible");
    expect(await resVisibility()).toBe("visible");
    await toggle(page, "overture_buildings_vn_south").uncheck();
    await expect.poll(resVisibility).toBe("none");
    await toggle(page, "overture_buildings_vn_south").check();
    await expect.poll(resVisibility).toBe("visible");
  });

  test("Settings lists every source with its licence link and required credit; says what the basemap server sees", async ({ page }) => {
    await page.getByTestId("settings").click();
    const credit = page.getByTestId("sources-credit");
    await expect(credit).toContainText("OpenStreetMap");
    await expect(credit).toContainText("Copernicus Digital Elevation Model");
    await expect(credit).toContainText("Foursquare");
    await expect(credit.locator("a[href^='https://']").first()).toBeVisible();
    await expect(page.locator(".popover")).toContainText("không được gửi cho nhà cung cấp bản đồ nền");
  });

  test("dataset search folds diacritics and hides the groups it empties", async ({ page }) => {
    const search = page.getByTestId("dataset-search");
    await search.fill("duong");
    await expect(page.locator('[data-testid="dataset"][data-id="overture_transport_hcm"]')).toBeVisible();
    await expect(page.locator('[data-testid="dataset"][data-id="overture_places_hcm"]')).toBeHidden();
    await expect(page.locator("aside.left .catalog-group:visible")).toHaveCount(1);
    await search.fill("");
    await expect(page.locator('[data-testid="dataset"][data-id="overture_places_hcm"]')).toBeVisible();
    await search.fill("zzzz-nothing");
    await expect(page.locator('[data-testid="dataset"]:visible')).toHaveCount(0);
  });

  test("the opacity slider restyles a layer without asking the server again", async ({ page }) => {
    await toggle(page, "overture_admin_hcm").check();
    await goTo(page, [106.70, 10.78], 12);
    await expect.poll(async () => (await page.evaluate(() => (window as unknown as { __gf: GF }).__gf.contextInfo()))["overture_admin_hcm"]?.features ?? 0, { timeout: 60_000 }).toBeGreaterThan(0);
    const before = await page.evaluate(() => (window as unknown as { __gf: GF }).__gf.state.logs.filter((l) => l.includes("POST /api/query")).length);
    const slider = page.locator('[data-testid="dataset"][data-id="overture_admin_hcm"] input[type="range"]');
    await slider.fill("30");
    await slider.dispatchEvent("change");
    await expect.poll(() => page.evaluate(() => (window as unknown as { __gf: GF }).__gf.map.getPaintProperty("ctx-overture_admin_hcm-line", "line-opacity"))).toBeCloseTo(0.3, 5);
    const after = await page.evaluate(() => (window as unknown as { __gf: GF }).__gf.state.logs.filter((l) => l.includes("POST /api/query")).length);
    expect(after).toBe(before);
  });
});

test.describe("demo workflows (Explorer, Vietnamese; every query goes through the gateway planner)", () => {
  test.beforeEach(async ({ page }) => {
    await openApp(page, { mode: "explorer", lang: "vi", source: "overture_buildings_vn_south" });
    test.skip(!(await packPresent(page)), "the Vietnam data pack is not in data/local");
  });
  const run = async (page: Page, id: string) => {
    await page.getByTestId("tab-query").click();
    await page.getByTestId(`demo-run-${id}`).click();
    await expect(page.getByTestId("demo-summary")).not.toContainText("Đang chạy", { timeout: 120_000 });
    await expect(page.getByTestId("demo-summary")).not.toContainText("Không chạy được");
    return page.getByTestId("demo-summary").innerText();
  };

  test("A: buildings in the view", async ({ page }) => {
    const s = await run(page, "a");
    expect(s).toMatch(/\d[\d.]* tòa nhà trong khu vực đang xem/);
    expect(await layerCount(page, "res-fill")).toBeGreaterThan(20);
  });

  test("B: buildings within 200 m of main roads (metric Transform + SpatialSemiJoin on the server, each building once)", async ({ page }) => {
    const s = await run(page, "b");
    expect(s).toMatch(/\d[\d.]* tòa nhà nằm trong bán kính 200 m/);
    expect(s).toContain("mỗi tòa nhà chỉ tính một lần");
    expect(s).toContain("EPSG:32648");
    expect(s).not.toMatch(/độ nên|xấp xỉ|bằng độ/);
    await page.getByTestId("tab-execution").click();
    await expect(page.getByTestId("exec-summary")).toBeVisible();
    // the plan ran on the server: the join cannot go to the browser
    const placement = await page.evaluate(() => (window as unknown as { __gf: { state: { last: { placement?: string } | null } } }).__gf.state.last?.placement);
    expect(placement).toBe("server");
  });

  test("C: buildings per ward, as a table", async ({ page }) => {
    const s = await run(page, "c");
    expect(s).toContain("Nhiều nhất");
    await expect(page.getByTestId("results-table")).toBeVisible();
    const head = await page.getByTestId("results-table").locator("thead").innerText();
    expect(head).toContain("Số lượng");
    expect(await page.getByTestId("results-table").locator("tbody tr").count()).toBeGreaterThan(3);
  });

  test("D: schools and hospitals in the view", async ({ page }) => {
    const s = await run(page, "d");
    expect(s).toMatch(/\d[\d.]* địa điểm: \d[\d.]* trường học và \d[\d.]* bệnh viện/);
  });

  test("E: terrain overlay from the Copernicus COG (display only)", async ({ page }) => {
    const s = await run(page, "e");
    expect(s).toMatch(/Độ cao từ -?[\d.]+ đến [\d.]+ m/);
    expect(s).toContain("chưa phải phép phân tích");
    const hasLayer = await page.evaluate(() => (window as unknown as { __gf: GF }).__gf.map.getStyle().layers.some((l) => l.id === "dem-copernicus_dem_hcm"));
    expect(hasLayer).toBe(true);
  });
});

test("English shows the same demos and groups", async ({ page }) => {
  await openApp(page, { mode: "explorer", lang: "en", source: "overture_buildings_vn_south" });
  test.skip(!(await packPresent(page)), "the Vietnam data pack is not in data/local");
  const names = await page.getByTestId("dataset").locator(".name").allInnerTexts();
  for (const n of ["Buildings", "Roads", "Administrative boundaries", "Places", "Land use", "Terrain elevation"]) expect(names).toContain(n);
  await page.getByTestId("tab-query").click();
  for (const id of ["a", "b", "c", "d", "e"]) await expect(page.getByTestId(`demo-${id}`)).toBeVisible();
});

test("Developer mode adds the benchmark datasets (Explorer never lists them)", async ({ page }) => {
  await openApp(page, { mode: "developer", lang: "en", source: "overture_buildings_vn_south" });
  test.skip(!(await packPresent(page)), "the Vietnam data pack is not in data/local");
  await expect(page.locator('[data-testid="dataset"][data-id="clustered_10mb_spatial"]')).toBeVisible();
  await expect(page.locator("aside.left .catalog-group", { hasText: "Benchmark" })).toBeVisible();
  const roads = page.locator('[data-testid="dataset"][data-id="overture_transport_hcm"]');
  await roads.getByRole("button", { name: /Details/ }).click();
  await expect(roads.getByTestId("dataset-detail")).toContainText("overture_transport_hcm");
  await expect(roads.getByTestId("dataset-detail")).toContainText("data/local/overture_transport_hcm.parquet");
  await expect(roads.getByTestId("dataset-detail")).toContainText("OGC:CRS84");
  await expect(roads.getByTestId("dataset-detail")).toContainText("Row groups");
  await openApp(page, { mode: "explorer", lang: "en", source: "overture_buildings_vn_south" });
  await expect(page.locator('[data-testid="dataset"][data-id="clustered_10mb_spatial"]')).toHaveCount(0);
});
