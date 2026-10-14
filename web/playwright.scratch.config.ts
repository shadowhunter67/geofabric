import { defineConfig } from "@playwright/test";
export default defineConfig({ testDir: "./e2e", testMatch: /zz-scratch\.spec\.ts/, workers: 1, reporter: [["list"]], use: { channel: "chrome", headless: true } });
