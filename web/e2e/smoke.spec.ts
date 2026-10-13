import { expect, test } from "@playwright/test";

// Types of the e2e hook exposed by the app when ?e2e=1 (test-only).
interface GF {
  map: {
    fitBounds(b: [[number, number], [number, number]], o: object): void;
    project(c: [number, number]): { x: number; y: number };
    once(ev: string, cb: () => void): void;
    triggerRepaint(): void;
  };
  state: { last: { geojson: { features: { geometry: { type: string; coordinates: number[][][] } }[] } | null } | null };
}
declare global {
  interface Window { __gf: GF }
}

test("core flow: map, sources, run, results, execution plan, inspector", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(String(e)));

  await page.goto("/?e2e=1");

  // 1. app loads, map container + canvas rendered
  await expect(page.locator("#map")).toBeVisible();
  await expect(page.locator("#map canvas.maplibregl-canvas")).toBeVisible();

  // 2. dataset listed (with CRS metadata)
  const source = page.locator("aside .item", { hasText: "clustered_10mb_spatial" });
  await expect(source).toBeVisible();
  await expect(source).toContainText("EPSG:4326");

  // 3. the first query runs automatically on selection; status shows rows + engine
  await expect(page.locator("header .status")).toContainText("rows ·", { timeout: 30_000 });
  await expect(page.locator("header .status")).toContainText("duckdb");

  // 4. Execution Plan tab: pipeline steps + metrics with provenance badges
  await page.getByRole("tab", { name: "Execution Plan" }).click();
  await expect(page.getByText("Read GeoParquet metadata")).toBeVisible();
  await expect(page.getByText("Compile logical plan")).toBeVisible();
  await expect(page.getByText("Planner decisions")).toBeVisible();
  const bytesRead = page.locator('[data-metric="Bytes read (actual I/O)"]');
  await expect(bytesRead).toHaveAttribute("data-source", "unknown");
  await expect(bytesRead).toContainText("Unknown");
  await expect(bytesRead).toContainText("Unavailable");
  const skipped = page.locator('[data-metric^="Row groups skipped"]');
  await expect(skipped).toHaveAttribute("data-source", "source_estimated");
  await expect(skipped).toContainText("Estimated");
  await expect(page.locator('[data-metric="Rows returned"]')).toHaveAttribute("data-source", "engine_observed");

  // 5. Results tab shows rows
  await page.getByRole("tab", { name: "Results" }).click();
  await expect(page.locator(".tabbody table th", { hasText: "category" })).toBeVisible();
  expect(await page.locator(".tabbody table tbody tr").count()).toBeGreaterThan(0);

  // 6. zoom into a real feature, press Run (viewport query), then click the feature
  const centre = await page.evaluate(() => {
    const f = window.__gf.state.last!.geojson!.features[0];
    const ring = f.geometry.coordinates[0];
    const xs = ring.map((p) => p[0]), ys = ring.map((p) => p[1]);
    return { w: Math.min(...xs), s: Math.min(...ys), e: Math.max(...xs), n: Math.max(...ys) };
  });
  await page.evaluate((b) => {
    window.__gf.map.fitBounds([[b.w, b.s], [b.e, b.n]], { padding: 220, maxZoom: 17, duration: 0 });
  }, centre);
  await page.getByRole("button", { name: "Run", exact: true }).click();
  await expect(page.locator("header .status")).toContainText("rows ·");
  await page.evaluate(() => new Promise<void>((res) => { window.__gf.map.once("idle", () => res()); window.__gf.map.triggerRepaint(); }));
  const px = await page.evaluate((b) => window.__gf.map.project([(b.w + b.e) / 2, (b.s + b.n) / 2]), centre);
  const box = (await page.locator("#map canvas.maplibregl-canvas").boundingBox())!;
  await page.mouse.click(box.x + px.x, box.y + px.y);

  // 7. Inspector opens with properties
  const inspector = page.locator("section.right");
  await expect(inspector).toContainText("Properties");
  await expect(inspector).toContainText("geometry type");
  await expect(inspector).toContainText("category");
  await expect(inspector).toContainText("EPSG:4326");
  await expect(inspector).toContainText("Lineage");

  expect(errors, `uncaught page errors: ${errors.join("; ")}`).toEqual([]);
});
