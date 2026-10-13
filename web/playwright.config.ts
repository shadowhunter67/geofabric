import { defineConfig } from "@playwright/test";

// Smoke test: starts the real gateway (API + built UI) and drives the system Chrome
// (no browser download). Needs: `npm run build`, benchmarks/data (10mb files).
//   cd web && npm run build && npx playwright test
const PORT = 8798;

export default defineConfig({
  testDir: "./e2e",
  timeout: 60_000,
  workers: 1, // one browser at a time (RAM discipline)
  retries: 0,
  reporter: [["list"]],
  use: { baseURL: `http://127.0.0.1:${PORT}`, channel: "chrome", headless: true, viewport: { width: 1500, height: 900 } },
  webServer: {
    // run from the repo root; builds the gateway on first use
    command: `cargo run -p geo-gateway -- --data benchmarks/data --web web/dist --port ${PORT}`,
    cwd: "..",
    url: `http://127.0.0.1:${PORT}/api/health`,
    reuseExistingServer: true,
    timeout: 900_000,
    env: { CARGO_BUILD_JOBS: "3" },
  },
});
