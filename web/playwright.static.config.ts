import { defineConfig } from "@playwright/test";

// Static browser profile (no Gateway, no PostGIS): serves the built bundle under the GitHub Pages base path and drives the system browser.
//   cd web && npx vite build --mode static --base /geofabric/ --outDir dist-static && GF_OUT=dist-static node scripts/copy-demo.mjs
//   npx playwright test -c playwright.static.config.ts
// PUBLIC_URL=https://<owner>.github.io/geofabric/ runs the very same spec against the deployed site instead (no local server).
const PUBLIC_URL = process.env.PUBLIC_URL;
const PORT = 4180;
const BASE = PUBLIC_URL ? PUBLIC_URL.replace(/\/?$/, "/") : `http://127.0.0.1:${PORT}/geofabric/`;

export default defineConfig({
  testDir: "./e2e-static",
  timeout: 120_000,
  workers: 1,
  retries: process.env.CI ? 1 : 0, // one retry on the hosted runner: one test depends on a public basemap server
  reporter: [["list"]],
  use: { baseURL: BASE, channel: (process.env.PW_CHANNEL ?? "chrome") as "chrome" | "msedge", headless: true, viewport: { width: 1500, height: 900 } },
  webServer: PUBLIC_URL
    ? undefined
    : { command: `node scripts/pages-like-server.mjs ${process.env.GF_OUT ?? "dist-static"} ${PORT}`, url: BASE, reuseExistingServer: false, timeout: 60_000 },
});
