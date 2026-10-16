import { test } from "@playwright/test";
test("dbg", async ({ page }) => {
  const errs: string[] = [];
  page.on("pageerror", (e) => errs.push("PAGEERROR " + String(e.stack)));
  page.on("console", (m) => { if (m.type() === "error") errs.push("CONSOLE " + m.text()); });
  await page.route(/tiles\.openfreemap\.org/, (r) => r.abort());
  await page.goto("/?e2e=1&mode=explorer&lang=vi&source=overture_buildings_vn_south");
  await page.waitForTimeout(8000);
  console.log("LOGS:" + JSON.stringify(await page.evaluate(() => (window as unknown as { __gf: { state: { logs: string[] } } }).__gf.state.logs.slice(-12))));
  console.log("ERRS:" + JSON.stringify(errs));
});
