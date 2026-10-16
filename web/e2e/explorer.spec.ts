import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import * as fs from "node:fs";
import { openApp, tab } from "./helpers.ts";

// Explorer / Developer product UI: language, progressive disclosure, layout at desktop sizes, accessibility.

const JARGON = /duckdb|wasm|postgis|plangraph|row group|credit|planner|confidence|health|probe|covering/i;

async function visibleText(page: Page, selector: string) {
  return page.locator(selector).evaluate((n) => (n as HTMLElement).innerText);
}

test.describe("Explorer mode (Vietnamese by default)", () => {
  test.beforeEach(async ({ page }) => { await openApp(page, { mode: "explorer", lang: "vi" }); });

  test("loads, lists friendly dataset names, no engine jargon anywhere on screen", async ({ page }) => {
    await expect(page.locator("header")).toContainText("Khám phá");
    const names = await page.getByTestId("dataset").locator(".name").allInnerTexts();
    expect(names).toContain("Tòa nhà — Nhỏ");
    expect(names).toContain("Tòa nhà Overture");
    expect(names.some((n) => /clustered|_/.test(n))).toBe(false);
    await expect(page.getByTestId("status")).toContainText("kết quả");
    await tab(page, "execution").click();
    const all = [await visibleText(page, "header"), await visibleText(page, "aside.left"), await visibleText(page, ".bottom")].join("\n");
    expect(all, "Explorer shows no engine or planner jargon").not.toMatch(JARGON);
  });

  test("execution summary: recommended place, reasons, numbers, alternative, optimisation; details on demand", async ({ page }) => {
    await tab(page, "execution").click();
    const s = page.getByTestId("exec-summary");
    await expect(s).toContainText("Lựa chọn được khuyến nghị");
    await expect(page.getByTestId("exec-where")).toHaveAttribute("data-where", /server|browser/);
    await expect(page.getByTestId("exec-where")).toContainText(/Xử lý trên (máy chủ|trình duyệt)/);
    await expect(s.getByTestId("stat-time")).toContainText(/ms|s/);
    await expect(s.getByTestId("stat-rows")).toBeVisible();
    await expect(s.getByTestId("exec-alt")).toContainText("Phương án khác");
    const opt = page.getByTestId("exec-optimize");
    await expect(opt).toContainText("Tối ưu truy vấn");
    // the first window covers the whole file: nothing could be skipped, and the summary says so honestly
    await expect(opt).toContainText(/Truy vấn này phải đọc gần như toàn bộ dữ liệu|Chỉ đọc dữ liệu cần thiết/);
    // a tight window prunes most of the file: the optimisation is described in plain words, with a percentage
    await page.evaluate(() => { const [x, y] = [104.5, 12.5]; window.__gf.map.fitBounds([[x - 0.03, y - 0.03], [x + 0.03, y + 0.03]], { duration: 0 }); });
    await page.getByTestId("run").click();
    await expect(page.getByTestId("status")).toContainText("kết quả");
    await tab(page, "execution").click();
    await expect(page.getByTestId("exec-optimize")).toContainText("Chỉ đọc dữ liệu cần thiết");
    await expect(page.getByTestId("exec-optimize")).toContainText("%");
    // advanced details are collapsed, then open on request
    await expect(page.getByTestId("advanced")).toBeHidden();
    await page.getByTestId("advanced-toggle").click();
    await expect(page.getByTestId("advanced")).toBeVisible();
    await expect(page.getByTestId("planner-panel")).toBeVisible();
    await expect(page.getByTestId("advanced-toggle")).toHaveAttribute("aria-expanded", "true");
  });

  test("pipeline strip tells the steps in plain words", async ({ page }) => {
    const nodes = page.getByTestId("pipe-node");
    expect(await nodes.count()).toBeGreaterThanOrEqual(4);
    const text = (await nodes.allInnerTexts()).join(" | ");
    expect(text).toContain("Nguồn dữ liệu");
    expect(text).toContain("Lọc theo vùng bản đồ");
    expect(text).toMatch(/Xử lý trên (máy chủ|trình duyệt)/);
  });

  test("dataset details expand on request; raw ids stay out of Explorer", async ({ page }) => {
    const item = page.locator('[data-testid="dataset"][data-id="clustered_10mb_spatial"]');
    await expect(item.getByTestId("dataset-detail")).toHaveCount(0);
    await item.getByRole("button", { name: /Chi tiết/ }).click();
    const d = item.getByTestId("dataset-detail");
    await expect(d).toContainText("Định dạng");
    await expect(d).toContainText("Hệ tọa độ");
    await expect(d).toContainText("Cập nhật");
    await expect(d).not.toContainText("clustered_10mb_spatial");
    await expect(d).not.toContainText("Row groups");
  });

  test("inspector is absent until a feature is selected; labels are friendly; history is plain", async ({ page }) => {
    await expect(page.locator("section.right")).toBeHidden();
    await page.evaluate(() => { window.__gf.select(window.__gf.state.last!.geojson!.features[0]); });
    const insp = page.locator("section.right");
    await expect(insp).toBeVisible();
    await expect(insp).toContainText("Đối tượng");
    await expect(insp).toContainText("Danh mục");
    await expect(insp).not.toContainText("Trường gốc");
    await expect(insp.getByTestId("history")).toContainText("Lọc theo vùng bản đồ");
    await expect(insp).not.toContainText("Lineage");
  });

  test("switching language keeps the layout and changes only the words", async ({ page }) => {
    const rects = async () => page.evaluate(() => Object.fromEntries(["header", "aside.left", "#map", ".bottom", ".tabs", ".pipeline"].map((s) => { const r = document.querySelector(s)!.getBoundingClientRect(); return [s, [Math.round(r.x), Math.round(r.y), Math.round(r.width), Math.round(r.height)]]; })));
    const vi = await rects();
    await page.getByTestId("lang").selectOption("en");
    await expect(page.getByTestId("tab-execution")).toHaveText("Execution");
    await expect(page.locator("header")).toContainText("Explore");
    const en = await rects();
    expect(en).toEqual(vi);
    await page.getByTestId("lang").selectOption("vi");
    await expect(page.getByTestId("tab-execution")).toHaveText("Thực thi");
  });

  test("Explorer hides developer tabs; Developer shows engines, scores, probe, provenance, IR, logs", async ({ page }) => {
    await expect(page.getByTestId("tab-profile")).toHaveCount(0);
    await page.getByTestId("mode-developer").click();
    await expect(page.getByTestId("tab-profile")).toHaveCount(1);
    await tab(page, "execution").click();
    await expect(page.getByTestId("engine-selected")).toBeVisible();
    await expect(page.getByTestId("assessment")).toBeVisible();
    await expect(page.getByTestId("confidence-badge")).toBeVisible();
    await expect(page.getByTestId("probe-badge")).toBeVisible();
    await expect(page.locator('[data-metric^="Row group"]').first()).toBeVisible();
    await tab(page, "query").click();
    await expect(page.getByTestId("ir")).toBeVisible();
    await tab(page, "logs").click();
    await expect(page.locator(".log")).toContainText(/plan|kế hoạch/);
    await expect(page.getByTestId("dev-strip")).toBeVisible();
    await page.getByTestId("mode-explorer").click();
    await expect(page.getByTestId("dev-strip")).toHaveCount(0);
  });
});

for (const size of [{ w: 1366, h: 768 }, { w: 1440, h: 900 }, { w: 1920, h: 1080 }]) {
  test(`layout at ${size.w}x${size.h}: the map keeps most of the screen, nothing overflows`, async ({ page }) => {
    await page.setViewportSize({ width: size.w, height: size.h });
    await openApp(page, { mode: "explorer", lang: "vi" });
    const g = await page.evaluate(() => {
      const r = (s: string) => document.querySelector(s)!.getBoundingClientRect();
      return { map: r("#map"), bottom: r(".bottom"), scrollW: document.documentElement.scrollWidth, scrollH: document.documentElement.scrollHeight, vw: innerWidth, vh: innerHeight };
    });
    expect(g.scrollW).toBeLessThanOrEqual(g.vw);
    expect(g.scrollH).toBeLessThanOrEqual(g.vh);
    expect(g.bottom.height).toBeLessThanOrEqual(g.vh * 0.4);
    expect(g.map.height).toBeGreaterThanOrEqual(g.vh * 0.5);
    expect(g.map.width).toBeGreaterThanOrEqual(g.vw * 0.55);
    await page.evaluate(() => { window.__gf.select(window.__gf.state.last!.geojson!.features[0]); });
    const m = await page.evaluate(() => document.querySelector("#map")!.getBoundingClientRect().width);
    expect(m).toBeGreaterThanOrEqual(size.w * 0.45);
  });
}

test("the bottom panel can be resized with the keyboard and stays within bounds", async ({ page }) => {
  await page.setViewportSize({ width: 1500, height: 900 });
  await openApp(page, { mode: "explorer", lang: "vi" });
  const h = () => page.evaluate(() => Math.round(document.querySelector(".bottom")!.getBoundingClientRect().height));
  const before = await h();
  await page.locator(".resizer").focus();
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("ArrowUp");
  expect(await h()).toBeGreaterThan(before);
  for (let i = 0; i < 40; i++) await page.keyboard.press("ArrowUp");
  expect(await h()).toBeLessThanOrEqual(Math.round(900 * 0.55) + 2);
  for (let i = 0; i < 60; i++) await page.keyboard.press("ArrowDown");
  expect(await h()).toBeGreaterThanOrEqual(170);
});

test.describe("accessibility", () => {
  test("every control has an accessible name; tabs follow the ARIA pattern; status is announced", async ({ page }) => {
    await openApp(page, { mode: "explorer", lang: "vi" });
    const unnamed = await page.evaluate(() => {
      const bad: string[] = [];
      document.querySelectorAll("button, select, input, textarea, [role=tab], [role=separator]").forEach((n) => {
        const e = n as HTMLElement;
        if (e.hidden || e.closest("[hidden]")) return;
        const name = (e.getAttribute("aria-label") ?? "") + (e.innerText ?? "") + (e.getAttribute("title") ?? "") + ((e as HTMLInputElement).labels?.[0]?.innerText ?? "");
        if (!name.trim()) bad.push(e.outerHTML.slice(0, 90));
      });
      return bad;
    });
    expect(unnamed, unnamed.join("\n")).toEqual([]);
    await expect(page.getByRole("tablist")).toBeVisible();
    await expect(page.getByRole("tab", { selected: true })).toHaveCount(1);
    await expect(page.getByTestId("status")).toHaveAttribute("aria-live", "polite");
    await expect(page.locator("#map")).toHaveAttribute("aria-label", /Bản đồ/);
    await expect(page.locator("header")).toHaveAttribute("role", "banner");
  });

  test("keyboard: arrows move between tabs, focus is visible", async ({ page }) => {
    await openApp(page, { mode: "explorer", lang: "vi" });
    await tab(page, "query").focus();
    await page.keyboard.press("ArrowRight");
    await expect(page.getByTestId("tab-results")).toBeFocused();
    await expect(page.getByTestId("tab-results")).toHaveAttribute("aria-selected", "true");
    const outline = await page.getByTestId("tab-results").evaluate((n) => getComputedStyle(n).outlineStyle + " " + getComputedStyle(n).outlineWidth);
    expect(outline).toMatch(/solid 2px/);
    await page.getByTestId("run").focus();
    await page.keyboard.press("Shift+Tab");
    await page.keyboard.press("Tab");
    await expect(page.getByTestId("run")).toBeFocused();
    expect(await page.getByTestId("run").evaluate((n) => getComputedStyle(n).outlineStyle)).toBe("solid");
  });

  test("status is conveyed with icon and text, not colour alone", async ({ page }) => {
    await openApp(page, { mode: "developer", lang: "en" });
    await tab(page, "execution").click();
    await expect(page.getByTestId("confidence-badge")).toContainText(/✓|⚠|✕/);
    await expect(page.getByTestId("safety-badge")).toContainText(/Safe|Risky|Unsafe|Unknown/);
    await page.getByTestId("mode-explorer").click();
    await expect(page.locator(".rec-head svg")).toHaveCount(1);
    await expect(page.locator(".rec-head")).toContainText("Recommended");
  });

  test("Escape closes settings and help", async ({ page }) => {
    await openApp(page, { mode: "explorer", lang: "vi" });
    await page.getByTestId("help").click();
    await expect(page.getByRole("dialog")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toBeHidden();
    await page.getByTestId("settings").click();
    await expect(page.getByRole("dialog")).toContainText("Cài đặt");
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toBeHidden();
  });
});

test("UI screenshots (UI_SHOTS=1): final Explorer vi/en, Developer, selected feature, execution, same layout", async ({ page }) => {
  test.skip(!process.env.UI_SHOTS, "set UI_SHOTS=1");
  test.setTimeout(240_000);
  await page.setViewportSize({ width: 1600, height: 900 });
  const out = "../docs";
  fs.mkdirSync(out, { recursive: true });
  type G = { map: { loaded: () => boolean; isMoving: () => boolean; once: (e: string, f: () => void) => void; triggerRepaint: () => void; project: (c: [number, number]) => { x: number; y: number }; getCanvas: () => HTMLCanvasElement; queryRenderedFeatures: (p: unknown, o?: unknown) => { geometry: GeoJSON.Geometry }[] } };
  const settle = async () => {
    // real basemap tiles (the one test that goes to the public tile server): wait for them, then a moment for the draw
    await page.evaluate(() => new Promise<void>((done) => { const m = (window as unknown as { __gf: G }).__gf.map; const t0 = Date.now(); const poll = () => (m.loaded() && !m.isMoving()) || Date.now() - t0 > 20_000 ? done() : setTimeout(poll, 200); poll(); }));
    await page.waitForTimeout(1200);
  };
  const open = async (mode: "explorer" | "developer", lang: "vi" | "en") => { await openApp(page, { mode, lang, tiles: "network", source: "overture_buildings_vn_south" }); await settle(); };
  const shot = (file: string) => page.screenshot({ path: `${out}/${file}` });

  await open("explorer", "vi");
  await tab(page, "execution").click();
  await expect(page.getByTestId("exec-summary")).toBeVisible({ timeout: 20_000 });
  await shot("ui-final-explorer-vi.png");
  await page.getByTestId("advanced-toggle").click();
  await page.waitForTimeout(300);
  await shot("ui-final-execution.png");

  await open("explorer", "en");
  await tab(page, "execution").click();
  await expect(page.getByTestId("exec-summary")).toBeVisible({ timeout: 20_000 });
  await shot("ui-final-explorer-en.png");

  await open("developer", "en");
  await tab(page, "execution").click();
  await expect(page.getByTestId("exec-summary")).toBeVisible({ timeout: 20_000 });
  await shot("ui-final-developer.png");

  // a real click on a drawn polygon
  await open("explorer", "vi");
  await tab(page, "execution").click();
  const at = await page.evaluate(() => {
    const g = (window as unknown as { __gf: G }).__gf;
    const fs = g.map.queryRenderedFeatures(undefined, { layers: ["res-fill"] });
    const f = fs[Math.floor(fs.length / 2)];
    const ring = (f.geometry as GeoJSON.Polygon).coordinates[0];
    const px = g.map.project([ring.reduce((a, p) => a + p[0], 0) / ring.length, ring.reduce((a, p) => a + p[1], 0) / ring.length]);
    const r = g.map.getCanvas().getBoundingClientRect();
    return { x: r.left + px.x, y: r.top + px.y };
  });
  await page.mouse.click(at.x, at.y);
  await expect(page.getByTestId("inspector-props")).toBeVisible();
  await page.mouse.move(5, 5);
  await settle();
  await shot("ui-final-feature-selected.png");
});
