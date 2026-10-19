// Runs the in-page device check (?devicecheck=1) in several real browser engines against ONE release gateway and saves
// each report under benchmarks/results/devicecheck/. Usage (from web/, Node 24 on PATH):
//   node ../benchmarks/run_devicecheck.mjs chrome msedge webkit
// The gateway is started here and stopped by PID. Nothing is killed by name. Same-machine evidence only: see the report.
import { spawn } from "node:child_process";
import { chromium, webkit, firefox } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";

const ROOT = path.resolve(import.meta.dirname, "..");
const PORT = 8823;
const OUT = path.join(ROOT, "benchmarks", "results", "devicecheck");
fs.mkdirSync(OUT, { recursive: true });
const lib = path.join(ROOT, "tools", ".cache", "duckdb-1.5.6", "lib");
const gw = spawn(path.join(ROOT, "target", "release", "geo-gateway.exe"),
  ["--data", "data/local", "--data", "benchmarks/data", "--manifests", "data/manifests", "--repo-root", ".", "--web", "web/dist", "--port", String(PORT), "--devicecheck-dir", OUT, "--no-learn"],
  { cwd: ROOT, env: { ...process.env, PATH: lib + path.delimiter + process.env.PATH }, stdio: "ignore" });
const base = `http://127.0.0.1:${PORT}`;
const up = async () => { for (let i = 0; i < 120; i++) { try { if ((await fetch(base + "/api/health")).ok) return true; } catch {} await new Promise((r) => setTimeout(r, 1000)); } return false; };
if (!(await up())) { gw.kill(); throw new Error("gateway did not start"); }

const kinds = { chrome: () => chromium.launch({ channel: "chrome" }), msedge: () => chromium.launch({ channel: "msedge" }), webkit: () => webkit.launch(process.env.WK_EXE ? { executablePath: process.env.WK_EXE } : {}), firefox: () => firefox.launch() };
const summary = {};
try {
  for (const name of process.argv.slice(2)) {
    const t0 = Date.now();
    let browser;
    try {
      browser = await kinds[name]();
      const page = await browser.newPage({ viewport: { width: 1400, height: 900 } });
      await page.goto(`${base}/?devicecheck=1&lang=en`);
      await page.waitForFunction(() => window.__devicecheck && window.__devicecheck.done, null, { timeout: 15 * 60_000 });
      const rep = await page.evaluate(() => window.__devicecheck);
      fs.writeFileSync(path.join(OUT, `matrix-${name}.json`), JSON.stringify(rep, null, 1));
      summary[name] = { seconds: Math.round((Date.now() - t0) / 1000), ua: rep.device.userAgent, results: rep.results.map((r) => ({ id: r.id, status: r.status, planner: r.planner && r.planner.placement, refused: r.browser_refused_correctly, digests: r.digests_match, note: r.note })) };
    } catch (e) {
      summary[name] = { error: String(e).slice(0, 300) };
    } finally {
      if (browser) await browser.close().catch(() => {});
    }
  }
} finally {
  gw.kill();
}
fs.writeFileSync(path.join(OUT, "summary.json"), JSON.stringify(summary, null, 1));
console.log(JSON.stringify(summary, null, 1));
