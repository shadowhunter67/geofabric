import { expect } from "@playwright/test";
import type { Page } from "@playwright/test";

/** Open the app and wait for the first analysis (it runs automatically when the first dataset is selected). */
export async function openApp(page: Page, o: { mode?: "explorer" | "developer"; lang?: "vi" | "en"; wait?: boolean; source?: string; tiles?: "blocked" | "network" } = {}) {
  // the basemap tiles come from a public server: tests never depend on it (map.spec.ts serves its own style)
  if (o.tiles !== "network") await page.route(/tiles\.openfreemap\.org/, (r) => r.abort());
  await page.goto(`/?e2e=1&mode=${o.mode ?? "developer"}&lang=${o.lang ?? "en"}&source=${o.source ?? "clustered_10mb_spatial"}`);
  if (o.wait !== false) {
    await expect(page.getByTestId("status")).toContainText(/results|kết quả/, { timeout: 60_000 });
    // opening a dataset = first query + (when its features are too small to see) a focus query on the densest area
    await page.waitForFunction(() => (window as unknown as { __gf: { state: { settling: boolean; busy: boolean } } }).__gf.state.settling === false, undefined, { timeout: 90_000 });
  }
}

export const tab = (page: Page, id: "query" | "results" | "execution" | "profile" | "logs") => page.getByTestId(`tab-${id}`);

/** Run a query template through the gateway from inside the page (same origin as the UI). */
export async function post(page: Page, path: string, body: unknown) {
  return page.evaluate(async ([p, b]) => {
    const r = await fetch(p as string, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(b) });
    return { status: r.status, json: await r.json() };
  }, [path, body] as const);
}
