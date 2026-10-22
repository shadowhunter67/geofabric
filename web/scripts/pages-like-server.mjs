// Static server for the static-mode browser tests. It behaves like GitHub Pages where it matters to DuckDB-Wasm: a HEAD request that carries
// a Range header is answered with 200 (no Content-Range), which makes DuckDB-Wasm unable to range-read (ADR-014). `vite preview` answers 206 and
// hid that bug until the site was deployed. GET honours Range (206). Serves <dir> under /geofabric/.
//   node scripts/pages-like-server.mjs <dir> [port] [--range-head]
import http from "node:http";
import { createReadStream, statSync, existsSync } from "node:fs";
import { join, extname, resolve } from "node:path";

const [dir, port] = [resolve(process.argv[2]), Number(process.argv[3] ?? 4190)];
// --range-head: answer a ranged HEAD like a range-capable host (206 + Content-Range), the behaviour jsDelivr / an object store has
const RANGE_HEAD = process.argv.includes("--range-head");
const TYPES = { ".html": "text/html; charset=utf-8", ".js": "application/javascript; charset=utf-8", ".mjs": "application/javascript; charset=utf-8", ".css": "text/css", ".json": "application/json", ".wasm": "application/wasm", ".parquet": "application/octet-stream", ".zip": "application/zip", ".png": "image/png" };

http.createServer((req, res) => {
  const url = new URL(req.url, "http://x");
  if (!url.pathname.startsWith("/geofabric/")) { res.writeHead(404).end("not found"); return; }
  let rel = decodeURIComponent(url.pathname.slice("/geofabric/".length)) || "index.html";
  if (rel.endsWith("/")) rel += "index.html";
  const file = join(dir, rel);
  if (!file.startsWith(dir) || !existsSync(file) || !statSync(file).isFile()) { res.writeHead(404).end("not found"); return; }
  const size = statSync(file).size;
  const headers = { "content-type": TYPES[extname(file)] ?? "application/octet-stream", "accept-ranges": "bytes", "access-control-allow-origin": "*", "cache-control": "max-age=600" };
  const range = req.headers.range?.match(/^bytes=(\d*)-(\d*)$/);
  if (req.method === "HEAD" && RANGE_HEAD && range) {
    const start = range[1] === "" ? 0 : Number(range[1]);
    const end = range[2] === "" ? size - 1 : Math.min(Number(range[2]), size - 1);
    res.writeHead(206, { ...headers, "content-range": `bytes ${start}-${end}/${size}`, "content-length": end - start + 1 }).end();
    return;
  }
  if (req.method === "HEAD") { res.writeHead(200, { ...headers, "content-length": size }).end(); return; } // like Pages: ranged HEAD -> 200
  if (range) {
    const start = range[1] === "" ? Math.max(0, size - Number(range[2])) : Number(range[1]);
    const end = range[1] === "" || range[2] === "" ? size - 1 : Math.min(Number(range[2]), size - 1);
    res.writeHead(206, { ...headers, "content-range": `bytes ${start}-${end}/${size}`, "content-length": end - start + 1 });
    createReadStream(file, { start, end }).pipe(res);
    return;
  }
  res.writeHead(200, { ...headers, "content-length": size });
  createReadStream(file).pipe(res);
}).listen(port, "127.0.0.1", () => console.log(`pages-like server on http://127.0.0.1:${port}/geofabric/ serving ${dir}`));
