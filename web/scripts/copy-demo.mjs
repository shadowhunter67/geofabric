// Put the public demo pack next to the built static site (<out>/demo). Cross-platform; fails loudly if the pack is missing.
//
//   default           the manifest (data/public-demo/pack.json) names a VERIFIED remote host (host.base_url): only the app-config files
//                     (pack.json, sources.json, catalog.json) are published with the site; the data is read from the host (HTTP range).
//                     No host in the manifest -> the data files are bundled with the site (offline/dev/CI), whole-file mode.
//   --bundle          always bundle the data (and drop `host` from the published manifest): a self-contained site, nothing remote.
//   --bundle-range    bundle the data but declare it range-capable: for local tests of range reads with
//                     `node scripts/pages-like-server.mjs <dir> <port> --range-head` (a server that answers ranged HEAD correctly).
// GF_OUT selects the output directory (default dist).
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const from = join(here, "..", "..", "data", "public-demo");
const to = join(here, "..", process.env.GF_OUT ?? "dist", "demo");
const wanted = process.env.GF_DEMO; // same switch for CI: GF_DEMO=bundle | bundle-range
const mode = process.argv.includes("--bundle-range") || wanted === "bundle-range" ? "bundle-range" : process.argv.includes("--bundle") || wanted === "bundle" ? "bundle" : "default";

if (!existsSync(join(from, "pack.json"))) {
  console.error(`public demo manifest not found at ${from}`);
  process.exit(1);
}
const manifest = JSON.parse(readFileSync(join(from, "pack.json"), "utf8"));
const remote = mode === "default" && manifest.host?.base_url;

rmSync(to, { recursive: true, force: true });
mkdirSync(to, { recursive: true });
for (const f of ["sources.json", "catalog.json"]) cpSync(join(from, f), join(to, f));

let published = manifest;
if (remote) {
  console.log(`data host: ${manifest.host.kind} @ ${String(manifest.host.commit).slice(0, 10)} (range=${manifest.host.range}); data NOT bundled`);
} else {
  if (!existsSync(join(from, manifest.version))) {
    console.error(`the data files (${manifest.version}/) are missing and the manifest names no host`);
    process.exit(1);
  }
  cpSync(join(from, manifest.version), join(to, manifest.version), { recursive: true });
  // a bundled copy reads relative to the page: no absolute URL and no remote host
  const files = Object.fromEntries(Object.entries(manifest.files).map(([id, f]) => [id, { ...f, url: undefined }]));
  published = { ...manifest, files, host: mode === "bundle-range" ? { kind: "bundled-range-test", base_url: "demo/", range: true } : null };
  console.log(`data bundled (${mode}); range=${published.host?.range ?? false}`);
}
writeFileSync(join(to, "pack.json"), JSON.stringify(published, null, 2) + "\n");
console.log(`demo pack -> ${to}`);
