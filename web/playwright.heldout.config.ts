import { defineConfig } from "@playwright/test";

// Held-out planner evaluation: talks to a gateway that is ALREADY running (started by benchmarks/run_heldout.ps1 with
// --simulate-network, --telemetry <calibration> and --no-learn). No webServer here.
//   HELDOUT=1 HO_BASE=http://127.0.0.1:8810 HO_NET=MOBILE HO_CASES=../benchmarks/results/heldout-cases.json \
//   HO_OUT=../benchmarks/results/heldout-MOBILE.json npx playwright test -c playwright.heldout.config.ts
export default defineConfig({
  testDir: "./e2e",
  testMatch: /heldout\.spec\.ts/,
  timeout: 60 * 60_000,
  workers: 1,
  retries: 0,
  reporter: [["list"]],
  use: { baseURL: process.env.HO_BASE ?? "http://127.0.0.1:8810", channel: "chrome", headless: true, viewport: { width: 1500, height: 900 } },
});
