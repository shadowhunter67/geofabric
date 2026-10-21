import { test } from "@playwright/test";

// README screenshots of the static browser profile (real product, real data, real basemap). Opt-in: UI_SHOTS=1.
test.skip(process.env.UI_SHOTS !== "1", "screenshots are opt-in (UI_SHOTS=1)");

async function ready(page: import("@playwright/test").Page) {
  await page.getByTestId("status").filter({ hasText: /results|kết quả/ }).waitFor({ timeout: 90_000 });
  await page.waitForFunction(() => (window as unknown as { __gf: { state: { settling: boolean } } }).__gf.state.settling === false, undefined, { timeout: 90_000 });
  await page.waitForFunction(() => (window as unknown as { __gf: { basemapLoaded: () => boolean } }).__gf.basemapLoaded(), undefined, { timeout: 60_000 }).catch(() => {});
  await page.evaluate(() => new Promise<void>((done) => {
    const m = (window as unknown as { __gf: { map: { once: (e: string, f: () => void) => void; loaded: () => boolean; triggerRepaint: () => void } } }).__gf.map;
    if (m.loaded()) setTimeout(done, 800); else { m.once("idle", () => setTimeout(done, 800)); m.triggerRepaint(); }
  }));
}

test("explorer (Vietnamese) and developer (English)", async ({ page }) => {
  await page.goto("./?e2e=1&mode=explorer&lang=vi");
  await ready(page);
  await page.screenshot({ path: "../docs/static-demo-explorer-vi.png" });
  await page.evaluate(() => (window as unknown as { __gf: { select: (f: unknown) => void; state: { last: { geojson: { features: unknown[] } } } } }).__gf.select(((window as unknown) as { __gf: { state: { last: { geojson: { features: unknown[] } } } } }).__gf.state.last.geojson.features[10]));
  await page.waitForTimeout(500);
  await page.screenshot({ path: "../docs/static-demo-inspector.png" });
  await page.goto("./?e2e=1&mode=developer&lang=en");
  await ready(page);
  await page.getByTestId("tab-execution").click();
  await page.waitForTimeout(500);
  await page.screenshot({ path: "../docs/static-demo-developer.png" });
});
