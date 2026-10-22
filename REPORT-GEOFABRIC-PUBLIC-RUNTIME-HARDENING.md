# GeoFabric — Public runtime hardening (2026-10-22)

Repository <https://github.com/shadowhunter67/geofabric> · Live demo <https://shadowhunter67.github.io/geofabric/>. Base `fd08f1c`; final commit see Repository State. Three priorities, in the brief's order: (1) the CI flake, (2) range-capable data delivery, (3) canonical lowering (RFC-004).

## Executive Summary

* **CI flake: root-caused and fixed, with controlled evidence.** The DuckDB profiler's JSON output made concurrent in-process engines fail sporadically with an opaque `Error code 1`. Same tests, same runner, only the profiler differs: **profiler on 8 of 40 runs failed, profiler off 0 of 40**. The profiler is now opt-in, peak memory is honestly `Unknown` by default, and the Rust gate runs in parallel again (the serial mitigation hid a real defect).
* **Range reads work on the public site.** The app stays on GitHub Pages; the data is served by jsDelivr's GitHub CDN at commit-pinned URLs. On the live site the first view makes **32 ranged requests, all `206`, no whole-file download, 4.2 MB of a 13.7 MB file**; first view total 13.7 MB (was ≈23 MB). No account, credential, billing or server was added.
* **Semantic duplication removed.** A pure crate `geo-lowering` now defines predicate trees, literal rules, `BboxFilter`, sort/NULL semantics, aggregates, the semi-join form, metres-only distances, CRS names and the row cap once. DuckDB and PostGIS are dialects over it; **the browser runs the same Rust compiled to WebAssembly and the TypeScript compiler (`sqlgen.ts`) is deleted**. Correctness unchanged: 17/17 plans identical on DuckDB and PostGIS after the refactor, Demo B equal to the independent oracle.
* Things that did not go to plan are in the sections below (a host that mixes backends, a build directory I committed by mistake and removed, two YAML escaping slips, the request→plan builder still duplicated).

## Repository State

`main`, normal commits only (no history rewrite). Working tree clean at the final commit. CI (`ci.yml`) and the Pages deployment green on the final commits; the deployed manifest names data commit `851b570` (immutable: a data change is a new `v2/` directory).

## CI Flake Investigation

Evidence collected, in order:
1. **Hosted-runner failures** always had the same shape: an opaque `Execution(Engine("Error code 1: Unknown error code"))` on the engine's query, in different DuckDB-heavy tests, mostly with parallel test threads.
2. **Diagnostics added** to the engine's failure path (`failure_context`): stage, query number (first vs later), thread, test threads, PID, timestamp, TEMP/TMP, cwd, DuckDB library path, the SQL head. No secrets. Every captured failure was `stage=execute`, a **later** query (2nd or 3rd), never the first: the "first DuckDB query" theory was wrong.
3. **Minimal repro** (`crates/geo-engine-duckdb/examples/duckdb_repro.rs`): only DuckDB — open, load `spatial`, write a deterministic Parquet fixture, apply the engine's session settings, run a first simple query, a first spatial query and a second one. CI matrix: threads 1/4 × fresh/pre-installed extension × arrow-first/lock. Result over three CI runs: **≈2,600 connections, 0 failures**. DuckDB start-up, extension install/load, the 8.3 short temp paths and the library path are cleared.
3b. A separate failure in one run was **not** this flake: `tools/duckdb_prebuilt.py` asked the GitHub API anonymously and got `HTTP 403 rate limit exceeded`, so `duckdb.lib` was missing (`LNK1181`). Fixed independently (the pinned digest needs no API call; authenticated otherwise).
4. **Parallel experiment** (`rust-parallel-experiment`, not a gate): the full suite with 4 threads, three attempts, printing diagnostics: failures reproduced there, absent in the bare repro ⇒ the trigger is in what the engine does on top of DuckDB.
5. **Controlled experiment** (`flake-experiment`): the two DuckDB-heavy test files run repeatedly with 4 test threads; only the DuckDB profiler (which the engine enables for the peak-memory metric, writing a JSON file per query) differs.

## CI Root Cause / Mitigation

| configuration | runs | failures |
|---|---|---|
| profiler on (engine default before) | 15 + 25 = 40 | **8** |
| profiler off | 15 + 25 = 40 | **0** |

Root cause (strongly correlated and removed by a specific change): **the DuckDB profiler's JSON output under concurrent in-process engines**. Why the profiler fails exactly this way is DuckDB's behaviour and not investigated further. Fix: `GEOFABRIC_DUCKDB_PROFILE=1` is now required to enable it (the benchmark tool sets it; it runs queries one at a time); otherwise `peak_memory_bytes` is `Unknown` with an explanatory note. The Rust gate dropped `--test-threads=1`. The experiment job stays as a regression monitor. Earlier real bugs fixed on the way: tests built paths from the 8.3 short temp directory (reproduced locally), a fresh-machine race on the `spatial` extension install (now serialized and retried, with attributable errors).

## Minimal DuckDB Repro

See above; runnable locally and in CI: `cargo run -p geo-engine-duckdb --example duckdb_repro --no-default-features -- --iters 200 --threads 4 --arrow-first [--lock]`.

## Public Data Hosting

Candidates tested with real requests (HEAD+Range, GET Range, preflight, browser-like `Accept-Encoding`):

| host | result |
|---|---|
| GitHub Pages | ranged `HEAD` → 200 (DuckDB-Wasm cannot range-read); gzips octet-stream |
| `raw.githubusercontent.com` | ranged `HEAD` → 200; preflight 403 |
| GitHub release assets | 302 to a signed URL; the redirect target answers 302 again on ranged HEAD: unusable |
| `cdn.jsdelivr.net` | multi-CDN name: **one backend (Fastly; also `fastly.jsdelivr.net`) answers ranged HEAD with 200**; others 206. A laptop got Cloudflare, the GitHub runner got Fastly, so the first manifest deployment failed its own gate. Not usable as a single name |
| **`gcore.jsdelivr.net`** | **206 on 12 of 12 requests**, correct `Content-Range`, no `Content-Encoding`, CORS `*` incl. preflight, immutable cache headers. **Chosen** |
| `testingcf.jsdelivr.net` | 206 12 of 12, but a test name: not used |

Cloudflare R2 or any account-based store was not used: it needs an account/billing decision, which the brief reserves. No always-on server was added.

## Range Capability Verification

`data/scripts/verify_range_host.py` checks, per file and against the app's origin: ranged HEAD → 206 with the right total, GET Range → 206 with exactly the bytes asked for and a `Content-Range` total equal to the manifest size, no `Content-Encoding` (a compressed representation would make ranges refer to compressed bytes), the Parquet magic at both ends, CORS (allow-origin and a preflight with `range,cache-control,if-match`), and the SHA-256 of the full served bytes. All five files pass on `gcore.jsdelivr.net`, from this machine and from the GitHub runner (the Pages workflow runs the verifier before every deployment and refuses to deploy otherwise).

## Demo Data Manifest

`data/public-demo/pack.json` schema 2 is the one place that names data: version `v1`, per file `path`, absolute `url`, `bytes`, `sha256`, `format`, `crs`, `rows`, `row_groups`, `license`, `source_release`, `derived_from`, plus `host` (`kind`, `cdn_host`, `base_url`, `commit`, `range`, `verified_at`). The page resolves every data URL from it (no URLs hard-coded in several files) and picks range vs whole-file access from `host.range`. Data semantics unchanged: the files are the same bytes as the previously bundled pack (SHA-256 asserted when migrating).

## Public Data Deployment

`git push` is the upload. `data/scripts/publish_demo_data.py`: validate (sizes, SHA-256, Parquet magic, metadata present, versioned path, CDN size limit, attribution page exists) → pin to the commit that last touched `v1/` (must already be on `origin/main`) → verify the host → write `host` into the manifest. Nothing is manual and no credential exists. If verification fails the manifest is not written.

## Before/After Network Performance

Live site, fresh browser context each run (cold HTTP cache), Chrome, this machine's network (Southeast Asia → CDN), 3 runs each; `before` = the previous deployment (whole-file, whole-extent first view).

| | before (whole-file) | after (range) |
|---|---|---|
| first view total transferred | ≈ 23 MB (app 0.4 + DuckDB-Wasm 8.3 + extension 0.7 + buildings file 13.6) | **13.7 MB** (data 4.2 MB) |
| data requests | 1 full download (4 before the in-page file pool) | 36: 32 ranged `206`, 4 `HEAD`, **0 whole-file** |
| time to first visible result | 5.7 s (5.7 / 5.7 / 22.6 cold) | 4.8–5.5 s (4.8 / 5.4 / 5.5) |
| second query, warm page | ≈ 1.3 s | 1.1–1.6 s |

Honest reading: part of the saving is the new start view (a neighbourhood-sized view of the pack's centre instead of fitting the whole extent, which touched every row group); with the same view, whole-file mode would still have downloaded 13.6 MB for the buildings layer versus 4.2 MB by range. The benefit grows with layer size; at 27 MB total it is modest in time. The in-browser test asserts it: every data GET is a ranged 206 and fewer than 25 % of the file's bytes cross the network.

## Shared Lowering Architecture

```
PlanGraph → capability validation (planner) → geo-lowering (pure; wasm32-capable) → DuckDB renderer | PostGIS renderer | Browser renderer (wasm)
```

## RFC-004 Decision

Updated and accepted (`rfcs/RFC-004-canonical-lowering.md`): chosen direction (one Rust crate, browser via WebAssembly), rejected alternatives (gateway-shipped SQL, Substrait, code generation, fixtures-only), migration stages, compatibility, test strategy, remaining debt.

## Canonical Representation

Decision functions, not a new tree (the PlanGraph already is the logical tree): `predicate` + `PredicateDialect`, `format_float`/`inline_literal`, `bbox_covering_overlap`, `order_by`, `aggregate_call`, `semi_join_exists`, `within_distance_meters`, `proj_name`, `row_cap`. Parameters: the DuckDB family inlines escaped literals; PostGIS binds typed parameters through the same predicate tree (which literal is bound is the dialect's job; the literal *rules* are shared). The IR stays 0.3.

## DuckDB Renderer

`geo-engine-duckdb/src/sql.rs` keeps its relational skeleton (authorization, covering discovery from Parquet footers, `ST_*` functions) and its private `lit`/`expr`/`proj_name`/aggregate/sort code is deleted.

## PostGIS Renderer

`geo-engine-postgis/src/lower.rs`: predicate tree through `PgPredicates` (binds leaves, refuses coercions), bbox exact part, sort (`COLLATE "C"` only for text keys), semi join, aggregate calls + PostgreSQL result casts, row cap, literal rules.

## Browser Renderer

`geo-lowering::browser` → `wasm32` (346 KB, 102 KB gzipped) → `web/src/engine/lowering.ts` (JSON across the boundary, no wasm-bindgen). `sqlgen.ts` deleted; `arch_check` forbids its return.

## Golden Fixtures

11 fixtures (`geo-engine-browser/tests/fixtures/lowering/*.json`) are read by the Rust reference test and by `web/test/lowering.test.ts`, which loads the real wasm and requires identical bytes. One fixture changed (sort text: `ASC NULLS LAST` now explicit). New `crates/geo-runtime/tests/canonical_lowering.rs` (8 tests): the same plan rendered by all three lowerings and compared with the canonical text (covering-box bbox and "not the geometry envelope" on PostGIS, NULLS LAST + collation, identical predicate trees, EXISTS + metres + explicit transforms, the browser's refusal of spatial operations, the angular-distance refusal, thinning, shared refusals).

## Cross-Engine Correctness

Live PostGIS after the refactor: 8/8 live driver tests; 17/17 plans identical (canonical digests) on the slice window; 16/16 identical on the 535 k-building wide window with Demo B ids equal to the independent shapely/pyproj oracle by SHA-256. (A first run on the wrong window failed the oracle comparison: the oracle is window-specific; the right window passed.) Metric semantics, tied `count_by`, BboxFilter and Transform cases are inside those 17 plans.

## Static Browser Regression

Static suite 13 tests (workflows on a second layer, partial reads when the host is range-capable, a layer whose host is unreachable gives a plain message while other layers work, server-only operations refused, Developer capability state, VI/EN, attribution page, network audit). Passed locally on Chrome, on a range-capable test server and a whole-file (Pages-like) one, and **on the live URL on Chrome and Edge**.

## Local Full-Stack Regression

Rust workspace 246 passed / 0 failed (was 228; +10 lowering unit, +8 canonical); web unit tests 95; `tsc` clean; `arch_check` OK with the new rules. Full-stack Playwright after the lowering refactor: Chrome 60 passed + the opt-in probe spec (a gitignored local probe, passes alone), Edge 61 passed; after the profiler change a 27-test smoke subset passed on a fresh gateway. PostGIS stays optional and unmeasured: R5d and the cost model were not touched.

## GitHub Actions

`ci.yml`: web (typecheck, unit tests, full + static builds, static browser tests against a bundled, range-capable test server and again against a whole-file one), architecture, Rust (parallel again), DuckDB repro matrix (4 jobs), parallel experiment and flake experiment (neither is a gate). `pages.yml`: builds the wasm and the site, verifies the data host from the runner, checks the bundle for private endpoints, deploys. Both install the `wasm32` target. CI never reads from the CDN.

## Public Deployment

Pages deployment green; manifest at `https://shadowhunter67.github.io/geofabric/demo/pack.json` names `gcore.jsdelivr.net` at commit `851b570`.

## Public Network Audit

Asserted by the live suite: hosts ⊆ {`shadowhunter67.github.io`, `*.openfreemap.org`, `extensions.duckdb.org`, `gcore.jsdelivr.net`}; no `localhost`, `127.0.0.1`, LAN, `:5433`, `postgres`; no `/api/*` request reaches the network.

## Architecture Risks

1. **Request → plan is still built twice** (Rust gateway templates and `web/src/static/ir.ts`): the next duplication to remove.
2. The data host is a third party with no SLA; mitigated by immutable commit-pinned URLs, verification before every deploy, and the friendly per-layer failure. The mixed-backend behaviour of `cdn.jsdelivr.net` shows such hosts can change; `gcore.jsdelivr.net` is a single-backend alias that could change too.
3. Relational skeletons (CTE chains, covering propagation) remain per engine; only decisions are shared.
4. The web build now needs the Rust toolchain and the wasm target.
5. Repo growth: the data lives in Git history (a new data version adds ~27 MB per version).

## Remaining P0

None.

## Remaining P1

Remove the request→plan duplication; a larger/second data pack on an object store if layers grow beyond what Git/jsDelivr suit; reconsider shipping the DuckDB extension instead of fetching it from DuckDB's CDN (not changed, documented); the cost model for PostGIS and R5d were deliberately left alone.

## Recommended Next Phase

Unify request→plan in `geo-lowering` (templates) and delete `static/ir.ts`; then the PostGIS cost model with selectivity inputs on top of the now-shared semantics. Wait for review.
