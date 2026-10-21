import { defineConfig } from "vite";

// Dev: UI on :5173, gateway on :8787 (cargo run -p geo-gateway).
export default defineConfig(({ mode }) => ({
  base: process.env.GF_BASE ?? "/",
  // `--mode static` builds the browser-only deployment (see src/static/profile.ts); GF_BASE=/geofabric/ sets the Pages path
  define: { __GF_PROFILE__: JSON.stringify(mode === "static" ? "static" : "full") },
  server: { proxy: { "/api": "http://127.0.0.1:8787" } },
  build: { chunkSizeWarningLimit: 1200 },
}));
