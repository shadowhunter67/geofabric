import { defineConfig } from "vite";

// Dev: UI on :5173, gateway on :8787 (cargo run -p geo-gateway).
export default defineConfig({
  server: { proxy: { "/api": "http://127.0.0.1:8787" } },
  build: { chunkSizeWarningLimit: 1200 },
});
