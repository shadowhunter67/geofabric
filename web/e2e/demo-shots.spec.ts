import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import * as fs from "node:fs";
import { openApp } from "./helpers.ts";

// Screenshots of the real-data demo (UI_SHOTS=1): real basemap tiles, real data pack. Written to docs/demo-*.png.

type GF = { map: { jumpTo: (o: unknown) => void; loaded: () => boolean; isMoving: () => boolean }; contextInfo: () => Record<string, { features: number; loading: boolean }> };

async function settle(page: Page) {
  await page.evaluate(() => new Promise<void>((done) => {
    const m = (window as unknown as { __gf: GF }).__gf.map;
    const t0 = Date.now();
    const poll = () => (m.loaded() && !m.isMoving()) || Date.now() - t0 > 25_000 ? done() : setTimeout(poll, 200);
    setTimeout(poll, 300);
  }));
  await page.waitForTimeout(2500);
}
async function runDemo(page: Page, id: string, tab?: "results" | "execution") {
  await page.getByTestId("tab-query").click();
  await page.getByTestId(`demo-run-${id}`).click();
  await expect(page.getByTestId("demo-summary")).toBeVisible({ timeout: 120_000 });
  await expect(page.getByTestId("demo-summary")).not.toContainText("Đang chạy", { timeout: 120_000 });
  await expect.poll(async () => Object.values(await page.evaluate(() => (window as unknown as { __gf: GF }).__gf.contextInfo())).every((c) => !c.loading), { timeout: 60_000 }).toBe(true);
  if (tab) await page.getByTestId(`tab-${tab}`).click();
  await settle(page);
}

test("demo screenshots (UI_SHOTS=1)", async ({ page }) => {
  test.skip(!process.env.UI_SHOTS, "set UI_SHOTS=1");
  test.setTimeout(600_000);
  await page.setViewportSize({ width: 1600, height: 900 });
  fs.mkdirSync("../docs", { recursive: true });
  await openApp(page, { mode: "explorer", lang: "vi", tiles: "network", source: "overture_buildings_vn_south" });
  const shot = (name: string) => page.screenshot({ path: `../docs/${name}.png` });

  await runDemo(page, "a", "execution");
  await shot("demo-buildings");
  await runDemo(page, "b", "execution");
  await shot("demo-transportation");
  await runDemo(page, "c", "results");
  await shot("demo-boundaries");
  await runDemo(page, "d", "execution");
  await shot("demo-poi");

  // all vector layers together
  await page.getByTestId("tab-query").click();
  for (const id of ["overture_transport_hcm", "overture_places_hcm", "overture_admin_hcm", "overture_landuse_hcm"]) {
    const box = page.locator(`[data-testid="dataset"][data-id="${id}"] input[type="checkbox"]`);
    if (!(await box.isChecked())) await box.check();
  }
  await page.evaluate(() => (window as unknown as { __gf: GF }).__gf.map.jumpTo({ center: [106.6985, 10.7765], zoom: 15.3 }));
  await expect.poll(async () => (await page.evaluate(() => (window as unknown as { __gf: GF }).__gf.contextInfo()))["overture_transport_hcm"]?.features ?? 0, { timeout: 60_000 }).toBeGreaterThan(50);
  await page.getByTestId("tab-execution").click();
  await settle(page);
  await shot("demo-multi-layer");

  await runDemo(page, "e", "execution");
  await shot("demo-raster");
});
