# GeoFabric — Public GitHub release + static browser-first demo (2026-10-21/22)

Repository: <https://github.com/shadowhunter67/geofabric> (PUBLIC) · Live demo: <https://shadowhunter67.github.io/geofabric/>

## Executive Summary

**GEOFABRIC SOURCE: PUBLIC · PUBLIC STATIC DEMO: LIVE · BROWSER-FIRST DEPLOYMENT: PROVEN · SERVER: OPTIONAL · POSTGIS: OPTIONAL.**

A visitor opens the URL and gets a real map of central Ho Chi Minh City with real Overture buildings, roads, places and boundaries, queried by DuckDB-Wasm in their own browser; they can inspect features, filter, count by category, switch Vietnamese/English and open Developer mode. No PostgreSQL, PostGIS, Gateway, tunnel or LAN endpoint is involved. The 10 static-mode browser tests pass against the deployed origin on Chrome and Edge, the full-stack suites and the Rust/TS/architecture checks are unchanged-green, and CI and the Pages deployment are green on GitHub.

Four things differ from what was planned and are reported, not hidden: (1) the demo data is a bundled ODbL subset, not read from Overture's bucket (the owner's preference), with the evidence why; (2) GitHub Pages cannot serve DuckDB-Wasm range reads, found only on the deployed site, so layers are downloaded whole (≤ 14 MB each, once per session); (3) the public history was rewritten (noreply author, scrubbed local paths), so SHAs differ from the private history; (4) CI needed several rounds on the hosted runner and one intermittent DuckDB failure was never root-caused (see Known Limitations).

## Repository State

Base before this phase: `main` @ `25e2781` (PostGIS live verification, approved). Public history: 63 commits, all authored by the GitHub noreply identity. Working tree clean; the original local history is kept on the local branch `backup/pre-public-history` (not pushed). Deployed commit at the time of the final live checks: `0f67fc5` (docs-only commit on top of `b7d8148`).

## GitHub Repository

`shadowhunter67/geofabric` was created **private** first so the workflows could run on real runners, then made public only after the audit passed. Visibility verified from GitHub state: `PUBLIC`; default branch `main`; licence detected `Apache-2.0` (dual files); homepage = the live URL; topics `gis, duckdb-wasm, geoparquet, maplibre, postgis, webassembly`; **private vulnerability reporting enabled** (SECURITY.md points to it). No GitHub Release was created.

## Git Remote

`origin = https://github.com/shadowhunter67/geofabric.git` (no remote existed before; nothing was overwritten). The public clone was produced by a history rewrite in a separate clone; the working repository was then reset onto it.

## Public Audit

**PASS.** Checks run on every revision of the intended public history and re-run on the final tree before the visibility change: secrets, personal data, binaries/runtime directories, large files, licence, data licences/attribution, third-party licences.

## Secret Scan

No dedicated scanner was installed (and none was installed for this); the scan used `git grep` over all 63 revisions: high-confidence token/key patterns (GitHub tokens, `sk-`, AWS, Slack, PEM private keys, Google API keys) — none; credentialed connection strings — only placeholders (`user:password`, `dev:dev`, `u:secret-pw`, `gf_reader:CHANGE_ME`, `nobody:SECRET-PW-123` in tests); the two **real** local PostgreSQL passwords (read from the local credentials file by the scan, never printed) — absent from every revision; `.env*` never tracked except the committed placeholder `.env.example`. Passwords are generated per machine outside the repo.

## Git History Audit

Found and fixed before publication: every commit carried the owner's personal email and the machine name as author/committer; a few logs/JSON/scripts carried the OS user name, the machine name and `C:\Users\<name>\…` paths. The owner chose a rewrite: `git filter-branch` in a separate clone, author/committer → `shadowhunter67 <97017475+shadowhunter67@users.noreply.github.com>`, tree filter replacing the user name/host name in text files. Re-scanned after: 0 occurrences of the email, the host name or the user-name path in any revision or commit message. Commit trailers (`Co-Authored-By`) preserved.

## Large File Audit

Largest tracked blobs: demo pack `demo_buildings.parquet` 13.7 MB, `demo_places.parquet` 9.3 MB, `demo_transport.parquet` 2.2 MB, `demo_admin.parquet` 2.2 MB, README screenshots ≤ 0.9 MB. No runtime/database binaries (`.runtime/`, PostgreSQL/PostGIS, pgdata, dumps), no `.exe/.dll/.zip`, no benchmark data (`benchmarks/data/` is ignored), no full data pack, no browser profile or log of the dev machine beyond small committed evidence logs. `.git` ≈ 36 MB.

## Software Licence

**MIT OR Apache-2.0** — chosen by the owner when asked (there was no recorded decision; `Cargo.toml` carried the string from the first scaffold only). `LICENSE-MIT` and `LICENSE-APACHE` (the official Apache-2.0 text, fetched from apache.org) added. Dependencies checked: Rust (241 crates) permissive (MIT/Apache/BSD/ISC/Zlib/Unlicense/CDLA-Permissive for root certificates); npm production tree (78 packages) MIT/ISC/BSD/Apache/0BSD/CC0 (one package without a `license` field is MIT per its LICENSE file).

## Data Licenses

The demo pack is a derived database of Overture release 2026-09-23.1 (ODbL / OpenStreetMap-derived; places per-source CDLA-Permissive-2.0/Apache-2.0/CC0). It is offered under the **ODbL 1.0** with the required attributions; the code licence does not apply to it. Changes from the originals are documented (spatial cut, small row groups, buildings flattened to 13 listed columns, no geometry or kept value altered). **Deviation from the owner's choice:** the owner picked "read from Overture's public bucket"; that is not viable: the bucket keeps only the two newest releases (two listed on 2026-10-21), so a pinned URL would break, and the app needs the normalized flat schema, not the nested raw one. CORS and Range work on that bucket, which is how a future remote-source pack could be added. See `docs/DATA-LICENSES.md`.

## Attribution

Map corner (attribution of the datasets in view + OpenFreeMap/OpenMapTiles/OpenStreetMap), Settings → data sources (licence links), and a published page `attribution.html` (generated verbatim from the catalog) with the ODbL notice, the basemap, the software credits and the privacy statement. A test asserts the page carries the ODbL notice.

## Public Demo Architecture

Deployment profile ADR-014: `vite build --mode static` defines `__GF_PROFILE__`; the page answers its own `/api/*` calls (`web/src/static/{profile,ir,gateway}.ts`) from files published beside it. Same UI, same PlanGraph (IR 0.3), same browser lowering and DuckDB-Wasm executor as the full stack; no parallel query architecture. Docs: README, `docs/deployment-profiles.md` (A static browser / B local full stack / C self-hosted), ADR-014.

## Static Browser Mode

Capabilities that are absent are not errors: DuckDB server and PostGIS are reported as "not available in this deployment" (Developer: candidate rows, health `unknown` with that reason, `postgis.configured=false`); Explorer says only "this demo runs entirely in the browser". Server-only analyses (`near`, `near_count`, `join_count_by`, `intersects_features`) answer 422 `server_required` — "This analysis requires GeoFabric Server." / "Tính năng này cần GeoFabric Server." — and their demos are visible but disabled; an explicit server-engine hint or declining the browser is also refused. Nothing is approximated; 200 m is never converted to degrees. No cost estimate is invented for the single engine (`score_ms`/`cost` null).

## Public Demo Data

`data/public-demo/` 26 MB (buildings 153,507 rows; transport 27,086; places 132,643; admin 55; land use 1,617) for one ~9 km × 8 km window of central Ho Chi Minh City (the Saigon River, District 1, Thu Thiem); SHA-256 per file in `pack.json`; rebuilt by `data/scripts/make_public_demo.py`. Not preloaded: a layer is fetched when first queried.

## External Data Origins

At runtime the page contacts only: the static host (`shadowhunter67.github.io`: app, wasm, data), `tiles.openfreemap.org` (basemap style/tiles/fonts) and `extensions.duckdb.org` (DuckDB-Wasm loads its `parquet` extension — code, not data — on first use). Verified HTTPS/CORS for the cross-origin ones; OpenFreeMap works from the public origin (a test loads it). The Overture bucket is not contacted.

## GitHub Actions

`ci.yml` (push/PR, `contents: read`): **web** (ubuntu: typecheck, unit tests, full and static production builds, static-mode browser tests with the runner's Chrome); **architecture** (`tools/arch_check.py`); **rust** (windows-latest: synthetic test data generation, sha256-verified prebuilt libduckdb, `cargo test --workspace --no-default-features -- --test-threads=2`, diagnostics step on failure). `pages.yml` (push/dispatch; permissions `contents: read, pages: write, id-token: write`): build from source, a bundle self-check for private endpoints, upload, deploy. Only first-party `actions/*`, `dtolnay/rust-toolchain`, `Swatinem/rust-cache` at pinned major versions; no secrets in YAML. Last runs: CI and Deploy green on `0f67fc5`.

## Static Build

`GF_BASE=/geofabric/ npm run build:static` (production, sub-path). Initial payload on the live origin: app 0.42 MB gzip, DuckDB-Wasm 8.3 MB (gzip on the wire), the parquet extension 0.7 MB, first layer 13.6 MB; the bundle self-check found no localhost/LAN/PostgreSQL strings (the DuckDB-Wasm binary itself contains the literal `127.0.0.1` as library text and is excluded).

## Deployment Platform

**GitHub Pages**, from the workflow (not a manual upload). Two Pages behaviours that blocked the first deployment and are documented in ADR-014: (1) a `HEAD` with `Range: bytes=0-` is answered `200` without `Content-Range` while DuckDB-Wasm needs `206` to open a file for range reads — so on Pages DuckDB-Wasm cannot range-read at all; (2) the CDN gzips `application/octet-stream` and serves ranges of the compressed bytes. Resolution within Pages: the engine's whole-file mode (fetch once, decoded transparently, registered in memory) with a bounded in-page file pool so a layer is downloaded once per session (it was downloaded four times per first view before the pool). A host that answers ranged `HEAD` correctly would allow true range reads by changing one constant. Cloudflare Pages/other hosts were not needed and not evaluated; no VPS, no tunnel.

## Public URL

<https://shadowhunter67.github.io/geofabric/> — HTTP 200, assets 200 with correct types (`application/wasm`, JS, CSS), `attribution.html` 200, data files 200 with `Accept-Ranges` and CORS headers.

## Public Network Audit

Run on the live origin (Chrome and Edge): hosts ⊆ {static host, `*.openfreemap.org`, `extensions.duckdb.org`}; no request matching `:8787`, `:8798`, `localhost`, `192.168.*`, `10.*`, `:5433`, `postgres`; no `/api/*` request reached the network (the page answered them itself).

## DuckDB-Wasm Verification

On the deployed origin: the wasm and worker download, the runtime probe passes (every required check), a real query reads a published Parquet file and returns geometry. Verified by the live-URL suite on both browsers.

## Map Verification

Real basemap (OpenFreeMap dark, attribution visible) with 4,191 real building polygons in the first view (screenshots in `docs/static-demo-*.png`; no debug bounding boxes; plain-background fallback when the basemap is blocked — the suite blocks it in most tests and passes).

## Browser Query Verification

Feature query (first view), `count_by category` over the places (aggregate, sorted, top 50, asserted descending), attribute filter path; result geometry rendered; feature selection opens the Inspector.

## Explorer

Opens in Explorer, Vietnamese by default; no engine jargon on screen (asserted); the alternative-engine line reads "not available: this demo runs entirely in the browser".

## Developer Mode

Shows DuckDB-Wasm as the selected engine and two alternatives (DuckDB server, PostGIS) as "not available in this deployment", PostGIS "not configured", the PlanGraph/SQL/planner panels; asserted.

## I18n

Vietnamese/English switching (including the server-needed sentences) asserted on the live site.

## Privacy

No analytics, cookies or tracking. Queries stay in the browser. The basemap host can see which tiles are viewed; DuckDB's CDN sees the extension download (an IP-level request, no GeoFabric data). Documented in README, SECURITY.md and `attribution.html`.

## Public Smoke Test

Clean browser contexts, no localhost: UI loads, map and basemap load, the public dataset loads, features draw, DuckDB-Wasm starts, a browser query works, feature click/Inspector work, VI/EN works, Developer mode works, server-only capability handled gracefully, attribution page served. 10 passed + 1 opt-in screenshot spec skipped, on Chrome and Edge, against `https://shadowhunter67.github.io/geofabric/`.

## Performance Smoke

Fresh browser context, real network (Southeast Asia → GitHub CDN), three runs after the fix: time to first visible result **5.7 s** (twice; one cold run 22.6 s while the CDN warmed), settled ≈ 5.9 s; wire bytes: app 0.42 MB + DuckDB-Wasm 8.3 MB + extension 0.7 MB + one 13.6 MB layer ≈ **23 MB total on first view**, a second query on the warm page ≈ 1.3 s. Before the file pool the same view transferred 54 MB (the 13.6 MB file four times); that is the defect the smoke test found. No strict gate; nowhere near hundreds of MB.

## Local Full-Stack Regression

Rust workspace 228 passed / 0 failed; `arch_check` OK; web unit tests 92 passed; `tsc` clean; full-stack Playwright against a fresh Gateway built from the final code: **Chrome 61 passed / 0 failed, Edge 61 passed / 0 failed** (4 skipped in each are the opt-in screenshot/perf specs, not counted as evidence); the Gateway starts with PostGIS unconfigured and with it unreachable (the PostGIS phase's checks are unchanged). No PostGIS benchmark or calibration was re-run (out of scope).

## Tests

Offline/mock vs live reported separately: live PostgreSQL tests were not run this phase and skip in CI (a skip is not evidence). Static-mode: 92 web unit tests (12 for the plan builder and gateway shim, 4 for the file pool) and the 10-test browser suite (local Pages-like server, deployed origin; Chrome + Edge). A Pages-like local test server (`web/scripts/pages-like-server.mjs`, HEAD+Range answered 200) is the default for the local static suite because `vite preview` answered 206 and hid the deployment bug.

## Known Limitations

* Layers are downloaded whole on first use (≤ 14 MB; ≈ 23 MB for the first view), not by HTTP range, because of Pages' range-probe behaviour.
* DuckDB-Wasm's `parquet` extension comes from DuckDB's CDN at runtime; the basemap from OpenFreeMap. If either is down, the demo degrades (plain background) or fails (extension).
* The browser engine runs a subset of the plan; metric distance, spatial joins and exact intersection need the server and are disabled in the public demo.
* **CI instability:** on the hosted Windows runner several tests failed in turn. Two causes were real and fixed: tests built paths from the 8.3 short temp directory (reproduced locally with a short `TEMP`; the guard's lexical check rightly refuses them) and a fresh-machine race on the DuckDB `spatial` extension install (now serialized and retried). One further intermittent failure — the very first DuckDB query of a test process returning an opaque `Error code 1` — was **not root-caused**: it did not reproduce locally under the emulated conditions (8.3 temp, empty extension directory, two test threads), stopped after the changes above (3 consecutive green runs), and could be a runner artifact. It recurred once more after the repository was public (a docs-only commit failed in `aggregate_group_by_matches_oracle_totals`; the diagnostics step showed the extension loaded and the semi-join tests green three times in isolation), so the Rust job now runs with `--test-threads=1`. Mitigations that remain: serial tests, the extension pre-install, stage-labelled engine errors, and a diagnostics step that prints the DuckDB probe on failure.
* Public history differs from the private one (SHAs changed); the unrewritten history stays only on the owner's machine.

## Architecture Risks

1. A TypeScript twin of part of the Rust request→plan builder (`static/ir.ts`): another compiler of "request → plan", covering only `features/count/count_by`; unit-tested against the real browser lowering, but RFC-004's shared lowering remains the structural fix (not started, by instruction).
2. The static profile depends on two third-party hosts (basemap, extension CDN) and on GitHub Pages' header behaviour, which can change; the live-URL suite is the guard.
3. Whole-file loading does not scale past tens of MB per layer; larger packs need a host with correct range support.
4. The CI Rust job is Windows-only (the verified libduckdb is the Windows build) and showed intermittent failures (above).

## Remaining P0

None for the release. Operational: the owner should check the private-vulnerability-reporting inbox and the Pages settings once.

## Recommended Next Phase

(1) Root-cause the intermittent first-query DuckDB failure on hosted runners (or move the Rust job to a Linux libduckdb build); (2) a data pack on a range-capable host (object store with CORS) to restore true range reads and allow a larger area; (3) only then the PostGIS cost model and RFC-004 shared lowering that were deliberately left alone. Do not start them before the Principal Architect review.
