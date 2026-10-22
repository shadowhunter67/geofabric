// Build the Rust lowering (crates/geo-lowering) to WebAssembly and place it where the web app and its tests load it:
//   web/src/engine/generated/geo_lowering.wasm   (git-ignored; always built from source)
// Needs: Rust + `rustup target add wasm32-unknown-unknown`. Skips the build when the output is newer than every input.
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readdirSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..", "..");
const out = join(here, "..", "src", "engine", "generated", "geo_lowering.wasm");

function newest(dir) {
  let t = 0;
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    t = Math.max(t, e.isDirectory() ? newest(p) : statSync(p).mtimeMs);
  }
  return t;
}

const inputs = ["geo-lowering", "geo-ir", "geo-types"].map((c) => join(root, "crates", c));
const stamp = Math.max(...inputs.map((d) => newest(join(d, "src"))), statSync(join(root, "Cargo.toml")).mtimeMs, statSync(join(root, "Cargo.lock")).mtimeMs);
if (existsSync(out) && statSync(out).mtimeMs > stamp && !process.argv.includes("--force")) {
  console.log("lowering wasm is up to date");
  process.exit(0);
}

const r = spawnSync("cargo", ["build", "-p", "geo-lowering", "--target", "wasm32-unknown-unknown", "--profile", "wasm", "--locked"], { cwd: root, stdio: "inherit"});
if (r.status !== 0) {
  console.error("\ncargo failed. The lowering is Rust compiled to WebAssembly: install Rust and run `rustup target add wasm32-unknown-unknown`.");
  process.exit(r.status ?? 1);
}
const built = join(process.env.CARGO_TARGET_DIR ?? join(root, "target"), "wasm32-unknown-unknown", "wasm", "geo_lowering.wasm");
mkdirSync(dirname(out), { recursive: true });
copyFileSync(built, out);
console.log(`lowering wasm -> ${out} (${(statSync(out).size / 1024).toFixed(0)} KiB)`);
