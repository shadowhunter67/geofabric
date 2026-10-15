import { defineConfig } from "@playwright/test";

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
      ? `${process.env.GATEWAY_BIN} --data benchmarks/data --web web/dist --port ${PORT}`
      : `cargo run -p geo-gateway --no-default-features -- --data benchmarks/data --web web/dist --port ${PORT}`,
    cwd: "..",
    url: `http://127.0.0.1:${PORT}/api/health`,
    reuseExistingServer: true,
    timeout: 900_000,
    env: { CARGO_BUILD_JOBS: "3" },
  },
});
