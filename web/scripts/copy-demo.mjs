// Copy the public demo pack (data/public-demo) next to the built static site (dist/demo). Cross-platform; fails loudly if the pack is missing.
import { cpSync, existsSync, rmSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const from = join(here, "..", "..", "data", "public-demo");
const to = join(here, "..", process.env.GF_OUT ?? "dist", "demo");
if (!existsSync(join(from, "pack.json"))) {
  console.error(`public demo pack not found at ${from} (run data/scripts/make_public_demo.py)`);
  process.exit(1);
}
rmSync(to, { recursive: true, force: true });
cpSync(from, to, { recursive: true });
console.log(`copied demo pack -> ${to}`);
