import { execFileSync } from "node:child_process";
import { mkdirSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { checkGatewayIdentity } from "./gateway-identity.ts";

// Runs after the gateway has been started by Playwright's webServer: verify it is the binary of this source tree and record its identity.
const root = resolve(import.meta.dirname, "..", "..");

function newestRustMtime(dir: string): number {
  let newest = 0;
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    if (e.name === "target" || e.name === "node_modules" || e.name.startsWith(".")) continue;
    const p = join(dir, e.name);
    if (e.isDirectory()) newest = Math.max(newest, newestRustMtime(p));
    else if (e.name.endsWith(".rs") || e.name === "Cargo.toml") newest = Math.max(newest, statSync(p).mtimeMs);
  }
  return newest;
}

export default async function globalSetup(config: { projects: { use: { baseURL?: string } }[] }) {
  if (process.env.GF_SKIP_IDENTITY_CHECK === "1") return;
  const base = config.projects[0]?.use.baseURL ?? "http://127.0.0.1:8798";
  const health = (await (await fetch(`${base}/api/health`)).json()) as { build?: { commit: string; dirty: boolean } };
  const head = execFileSync("git", ["rev-parse", "HEAD"], { cwd: root }).toString().trim();
  const bin = process.env.GATEWAY_BIN ? resolve(root, process.env.GATEWAY_BIN) : undefined;
  const verdict = checkGatewayIdentity({
    build: health.build,
    head,
    binaryMtimeMs: bin ? statSync(bin).mtimeMs : undefined,
    newestSourceMtimeMs: Math.max(newestRustMtime(join(root, "crates")), statSync(join(root, "Cargo.lock")).mtimeMs),
  });
  mkdirSync(join(root, "web", "test-results"), { recursive: true });
  writeFileSync(join(root, "web", "test-results", "gateway-identity.json"), JSON.stringify({ head, build: health.build ?? null, binary: bin ?? "cargo run (built on start)", ...verdict }, null, 2));
  for (const n of verdict.notes) console.log(`gateway identity: ${n}`);
  if (!verdict.ok) throw new Error(`STALE GATEWAY: ${verdict.problems.join("; ")}. Stop the old gateway by its PID, rebuild, and rerun (GF_SKIP_IDENTITY_CHECK=1 to override).`);
  console.log(`gateway identity OK (${health.build?.commit.slice(0, 10)}${health.build?.dirty ? ", dirty" : ""})`);
}
