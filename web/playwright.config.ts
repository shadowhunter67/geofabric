import { defineConfig } from "@playwright/test";
import { gatewayEnv } from "./e2e/gateway-env.ts";

// Smoke test: starts the real gateway (API + built UI) and drives the system Chrome
// (no browser download). Needs: `npm run build`, benchmarks/data (10mb files).
//   cd web && npm run build && npx playwright test
const PORT = 8798;

export default defineConfig({
  testDir: "./e2e",
  testIgnore: /(heldout|envelope|crossover)\.spec\.ts/,
  timeout: 60_000,
  workers: 1, // one browser at a time (RAM discipline)
  retries: 0,
  reporter: [["list"]],
  use: { baseURL: `http://127.0.0.1:${PORT}`, channel: (process.env.PW_CHANNEL ?? "chrome") as "chrome" | "msedge", headless: true, viewport: { width: 1500, height: 900 } },
  webServer: {
    // run from the repo root; builds the gateway on first use
    // GATEWAY_BIN=target/release/geo-gateway.exe for timing runs (release build, no cargo)
    command: process.env.GATEWAY_BIN
      ? `${process.env.GATEWAY_BIN} --data data/local --data benchmarks/data --manifests data/manifests --repo-root . --web web/dist --port ${PORT}`
      : `cargo run -p geo-gateway --no-default-features -- --data data/local --data benchmarks/data --manifests data/manifests --repo-root . --web web/dist --port ${PORT}`,
    cwd: "..",
    url: `http://127.0.0.1:${PORT}/api/health`,
    // a FRESH gateway by default: a gateway left running (possibly with PostGIS configured) must not decide what the suite sees. PW_REUSE_GATEWAY=1 opts in.
    reuseExistingServer: process.env.PW_REUSE_GATEWAY === "1",
    timeout: 900_000,
    // GEOFABRIC_POSTGIS_* is blanked for this gateway unless GF_E2E_POSTGIS=1 (see e2e/gateway-env.ts and test/e2e-env.test.ts)
    env: gatewayEnv(),
  },
});
