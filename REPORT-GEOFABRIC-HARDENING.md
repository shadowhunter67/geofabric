# REPORT — GeoFabric Core MVP Hardening

Prepared for the Principal Architect. Date: 2026-10-13. One Windows 11 machine (28 logical cores, 32 GB RAM); nothing here is verified on another machine. No feature work was done in this round (no real-data download, no 10 GB benchmark, no second real engine).

## 1. Executive Summary

**Hardening status: all items of the release gate are implemented and verified, with the limitations listed below.**

Blockers resolved:
- **GeoParquet CRS semantics** now follow the spec (missing key → OGC:CRS84, `null` → Undefined, object → parsed) with a new `Crs` representation; never silently EPSG:4326.
- **Metrics have provenance**; the misleading `bytes read` number is gone (Unknown). UI shows Observed / Estimated / Derived / Unavailable badges.
- **Timeout and cancellation are real**: DuckDB's `interrupt()` is called by an engine-side watchdog; verified at engine level and through the gateway (timeout → 504, client disconnect → query unwinds, slot released, runtime reusable).
- **Authorization happens before any metadata I/O** (`SourceAccessGuard` → `AuthorizedSource`), with opaque denials.
- **Spatial join has an exact-pair correctness test**; benchmarks at 10 MB / 100 MB / 1 GB are validated against an independent oracle (84/84 runs pass).
- Gateway tests (8 scenarios) and a browser smoke test (map, Results, Execution Plan, Inspector) exist and pass.

Remaining blockers / limits (none is a release-gate item, all are honest limitations): actual I/O is still not measurable; synthetic data only; one real engine; the e2e smoke test is a single flow; no CI.

**Recommendation: APPROVE CORE MVP** — see §18 for the exact scope of that statement and the caveats.

## 2. Repository State

- Branch `main`; head {{COMMIT}}; previous `385a7a0` (hardening code), before that `03aaca8` (MVP report), `e49c4da` (MVP code), `d66684a` (architecture bootstrap).
- Tests: **66 passing** (was 44) + 1 browser smoke test. `tools/arch_check.py` passing (now also two source-text rules).

## 3. CRS Semantics

| GeoParquet metadata | GeoFabric `Crs` | CRS-sensitive ops |
|---|---|---|
| `crs` key **absent** | `OgcCrs84` | allowed; **not** equal to `Epsg(4326)` (axis order) → bbox/literal/join in EPSG:4326 vs a CRS84 column is a `crs_mismatch`, an explicit transform is required |
| `"crs": null` | `Undefined` | rejected (`unknown_crs`) |
| object with `id` EPSG:n | `Epsg(n)` | allowed |
| object with `id` OGC:CRS84 | `OgcCrs84` | allowed |
| string `"EPSG:4326"` / other | `Epsg` / `Unknown` | `Unknown` rejected |

Enum: `Crs { Epsg(u32), OgcCrs84, Undefined, Unknown(String) }`; `is_known()` = Epsg or CRS84; CRS84 counts as geographic. Display/parse round-trip is tested. Tests (all pass): missing key → CRS84 and not EPSG:4326, query in 4326 against it → mismatch, CRS84 query prunes; `null` → Undefined and pruning refused; explicit EPSG / OGC / other / string forms; IR: metric buffer and bbox on `Undefined` → `unknown_crs`; CRS84 vs 4326 mismatch; metric buffer on CRS84 → `metric_on_geographic`. Docs updated: RFC-001, ADR-003. The UI accepts `EPSG:4326` and `OGC:CRS84` sources for viewport queries.

## 4. Metrics Contract

```
Metric<T> { value: Option<T>, source: MetricSource }
MetricSource = EngineObserved | SourceObserved | SourceEstimated | PlannerEstimated | Derived | Unknown
```

| metric | value / provenance (DuckDB adapter) |
|---|---|
| rows returned, engine time | EngineObserved |
| row groups total | SourceObserved (footer) |
| row groups skipped | **SourceEstimated** — GeoFabric's analysis of footer statistics; DuckDB does not report its own pruning |
| rows scanned, bytes: upper bounds | SourceEstimated (`rows_scanned_upper_bound`, `estimated_bytes_upper_bound`) |
| **bytes read (actual I/O)** | **Unknown, `value: null`** — DuckDB's `total_bytes_read` under-counts (56 KB for a 98 MB scan) and is deliberately not reported |
| peak memory | EngineObserved, DuckDB buffer-manager peak only (not RSS) |

UI: every card carries a badge (Observed / Estimated / Derived / Unavailable); `Bytes read (actual I/O)` shows **Unknown / Unavailable**, the bound is shown separately as `Bytes read, upper bound ≤ …` with an Estimated badge. The estimate is never displayed as an observed value. Tests: gateway test asserts the provenance strings and `bytes_read.value == null`; the browser smoke test asserts the badges. RFC-002 was rewritten: engines may omit metrics; unknown is a first-class value. Benchmark JSON field names carry the provenance (`*_source_estimated`, `*_engine_observed`) and `bytes_read` is always `null`.

## 5. Timeout / Cancellation

- **Mechanism:** `ExecutionContext { timeout, cancel: CancelToken }`. The DuckDB adapter starts a watchdog thread per execution; when the deadline passes or the token is set it calls `InterruptHandle::interrupt()` every 10 ms until the query unwinds, then reports `EngineError::Timeout { limit_ms }` or `Cancelled`. This is a **real interruption of the running query**, not an HTTP-level timeout.
- **Default:** `limits::DEFAULT_QUERY_TIMEOUT = 30 s`, defined once in `geo-executor::limits`; gateway option `--timeout-secs`; no hard-coded values elsewhere (the benchmark runner sets its own 600 s explicitly).
- **Client timeout vs execution timeout:** execution timeout → engine interrupt → HTTP **504** `execution_timeout`. Client timeout/disconnect → the request future is dropped → a drop guard sets the cancel token → engine interrupt. The concurrency slot is held by the blocking task and is released only when the query has really finished.
- **Verified** (heavy self-join of the 100 MB dataset): engine level — timeout at 300 ms returns `Timeout`, completes well under 8 s, and the same runtime then serves a normal query; cancel token fired after 300 ms returns `Cancelled`; an already-cancelled context never starts. Gateway level — 400 ms timeout → 504, slot count restored, next query OK; client abandons after 300 ms → slot is freed within 15 s although the configured timeout is 120 s (so only the cancel path can explain it).
- **Limits:** granularity is the DuckDB interrupt check (not measured finer than "well under 8 s"); planning/footer reads before execution are not covered by the timeout (they are small); CPU-time or memory-kill isolation is not implemented (memory is bounded by DuckDB `memory_limit`).

## 6. Authorization Boundary

```
authorize(uri)  →  AuthorizedSource  →  metadata read (footer)  →  planner  →  executor
 (lexical check,     (only the guard           (open_authorized)
  no FS access        can construct it)
  outside the root)
```

`geo_types::SourceAccessGuard::authorize(uri)`: rejects URLs/empty/NUL, normalises the path **lexically**, and rejects anything outside the allowed roots **before touching the file system**; only paths inside a root are canonicalised (symlink escapes are re-checked). Denials are opaque (`source not permitted`): an existing file and a missing file outside the root produce byte-identical errors and responses (tested). The runtime authorizes every source of a plan before planning; the planner's scan analysis, `open_source`, the gateway listing/row-group endpoints and the engine all take an `AuthorizedSource`. `GeoParquetSource::open` (unchecked) remains only for tests/tooling, and `tools/arch_check.py` fails the build if production crates call it. DuckDB itself is confined to `allowed_paths = [the authorized files]` with external access disabled.

Tests: guard unit tests (outside/inside, `..`, URLs, root itself, missing file); planner test with a valid file and a non-existent path outside the root → `PlanError::AccessDenied` with identical text; gateway test (IR run/explain for existing vs missing outside file, `..`, `http://`, `s3://` → 403, identical bodies). Limitation: a time-based side channel (canonicalisation only happens for in-root paths) was not analysed.

## 7. Spatial Join Correctness

Dataset (`crates/geo-runtime/tests/join.rs`, written in-test): left = 4 polygons (two touching, one isolated, one overlapping a big polygon), right = 5 geometries (point inside, point on the shared edge, big polygon, far point, polygon touching an edge). Expected inner pairs `(1,1) (1,2) (2,2) (2,3) (2,5) (3,3)`; left join adds `(4,NULL)`. The test compares the **exact sorted pair list** (not a count), covers boundary-touch semantics and the left-join null row, determinism across two runs, and `Intersects` with a literal plus `BboxFilter` on a dataset **without** a bbox covering column (the `ST_Envelope` path). Result: **4/4 pass**. The old vacuous test (`n >= 0`) was removed. Join counts are also checked by the benchmark oracle on 100 MB and 1 GB (non-zero counts 11, 90, 132, 1989; the 10 MB clustered case is 0, so it is not discriminating).

## 8. Benchmark Correctness

Independent oracle: `benchmarks/oracle.py` uses **pyarrow + numpy only** — no DuckDB, no GeoFabric reader/IR/planner, and it does not use the bbox covering column (it decodes extents from the WKB bytes). Hash = SHA-256 over sorted canonical row strings (`|`-joined cells; ints decimal, strings raw, WKB lowercase hex; count queries = the count). The Rust harness computes the same digest from the Arrow result and compares with `expected-<size>.json`; any mismatch fails the run (exit 1). Gate result: **84 / 84 runs pass** (10 MB: 28, 100 MB: 28, 1 GB: 28); spatial and shuffled files of each pair return identical count and hash (42/42 pairs).

| size | dist | query | result count | result hash (12) | oracle | spatial=shuffled |
|---|---|---|---|---|---|---|
| 10mb | clustered | bbox_count_small(0.6%) | 33 | c6f3ac57944a | pass/pass | yes |
| 10mb | clustered | bbox_count_large(25%) | 26255 | ad979145f08b | pass/pass | yes |
| 10mb | clustered | attr_filter_fullscan | 2479 | cc0f20917ee0 | pass/pass | yes |
| 10mb | clustered | intersects_count_small | 33 | c6f3ac57944a | pass/pass | yes |
| 10mb | clustered | aggregate_by_category_small | 8 | 333ce7595d2a | pass/pass | yes |
| 10mb | clustered | spatial_join_small | 0 | 5feceb66ffc8 | pass/pass | yes |
| 10mb | clustered | return_rows_small(id,geometry) | 33 | 650ff1605209 | pass/pass | yes |
| 10mb | uniform | bbox_count_small(0.6%) | 844 | db9351a297a7 | pass/pass | yes |
| 10mb | uniform | bbox_count_large(25%) | 32389 | bb91daa56789 | pass/pass | yes |
| 10mb | uniform | attr_filter_fullscan | 2530 | 0e1a3aec7fd9 | pass/pass | yes |
| 10mb | uniform | intersects_count_small | 844 | db9351a297a7 | pass/pass | yes |
| 10mb | uniform | aggregate_by_category_small | 8 | f55e84dd9cd2 | pass/pass | yes |
| 10mb | uniform | spatial_join_small | 8 | 2c624232cdd2 | pass/pass | yes |
| 10mb | uniform | return_rows_small(id,geometry) | 844 | 314a02c26f6e | pass/pass | yes |
| 100mb | clustered | bbox_count_small(0.6%) | 350 | deeeb5df3f2c | pass/pass | yes |
| 100mb | clustered | bbox_count_large(25%) | 260786 | b70e48e814d1 | pass/pass | yes |
| 100mb | clustered | attr_filter_fullscan | 24839 | 3c6b965a43b7 | pass/pass | yes |
| 100mb | clustered | intersects_count_small | 350 | deeeb5df3f2c | pass/pass | yes |
| 100mb | clustered | aggregate_by_category_small | 8 | 133341cdf27e | pass/pass | yes |
| 100mb | clustered | spatial_join_small | 11 | 4fc82b26aecb | pass/pass | yes |
| 100mb | clustered | return_rows_small(id,geometry) | 350 | a72a60e96ef6 | pass/pass | yes |
| 100mb | uniform | bbox_count_small(0.6%) | 8184 | 0e5275f34e63 | pass/pass | yes |
| 100mb | uniform | bbox_count_large(25%) | 324751 | 1d54b73bb6b1 | pass/pass | yes |
| 100mb | uniform | attr_filter_fullscan | 24933 | 382bb35d59bd | pass/pass | yes |
| 100mb | uniform | intersects_count_small | 8184 | 0e5275f34e63 | pass/pass | yes |
| 100mb | uniform | aggregate_by_category_small | 8 | 5774f92579c1 | pass/pass | yes |
| 100mb | uniform | spatial_join_small | 90 | 69f59c273b6e | pass/pass | yes |
| 100mb | uniform | return_rows_small(id,geometry) | 8184 | 9a4dffbc988c | pass/pass | yes |
| 1gb | clustered | bbox_count_small(0.6%) | 3507 | 736eb54066e3 | pass/pass | yes |
| 1gb | clustered | bbox_count_large(25%) | 2611339 | 8aae96d082ed | pass/pass | yes |
| 1gb | clustered | attr_filter_fullscan | 248893 | 66877cb8d61e | pass/pass | yes |
| 1gb | clustered | intersects_count_small | 3507 | 736eb54066e3 | pass/pass | yes |
| 1gb | clustered | aggregate_by_category_small | 8 | 819840015694 | pass/pass | yes |
| 1gb | clustered | spatial_join_small | 132 | dbb1ded63bc7 | pass/pass | yes |
| 1gb | clustered | return_rows_small(id,geometry) | 3507 | aa53d54635e0 | pass/pass | yes |
| 1gb | uniform | bbox_count_small(0.6%) | 81698 | 57ce9c7f6da4 | pass/pass | yes |
| 1gb | uniform | bbox_count_large(25%) | 3251942 | 7a2a8bdc46d9 | pass/pass | yes |
| 1gb | uniform | attr_filter_fullscan | 249412 | f07223a53368 | pass/pass | yes |
| 1gb | uniform | intersects_count_small | 81698 | 57ce9c7f6da4 | pass/pass | yes |
| 1gb | uniform | aggregate_by_category_small | 8 | 67ead429829a | pass/pass | yes |
| 1gb | uniform | spatial_join_small | 1989 | 9113b98df80f | pass/pass | yes |
| 1gb | uniform | return_rows_small(id,geometry) | 81698 | 564221253a14 | pass/pass | yes |


Validation method note: the oracle shares the *definition* of the queries (it is written to the same spec), so it detects executor/engine bugs but not a misunderstanding of the spec itself; geometry is axis-aligned rectangles, so intersects == closed-interval overlap.

## 9. Performance

Release build, 3 timed repetitions after one warm-up for 10 MB/100 MB, 2 for 1 GB; median ms. `rg skipped` and `bytes ≤` are **statistics estimates**, `ms` is engine-observed; **actual bytes read is unknown**. DuckDB's own peak buffer memory stayed ≤ ~91 MB in every run. Only valid because §8 passed.

| size | dist | query | rg skipped (est.) spatial | rg skipped (est.) shuffled | bytes <= (est.) spatial | bytes <= (est.) shuffled | ms spatial (obs.) | ms shuffled (obs.) | speedup |
|---|---|---|---|---|---|---|---|---|---|
| 10mb | clustered | bbox_count_small(0.6%) | 23/26 | 0/26 | 1.1 MB | 10.0 MB | 4.8 | 7.3 | 1.5x |
| 10mb | clustered | bbox_count_large(25%) | 18/26 | 0/26 | 3.0 MB | 10.0 MB | 5.7 | 9.0 | 1.6x |
| 10mb | clustered | attr_filter_fullscan | 0/26 | 0/26 | 9.8 MB | 10.0 MB | 4.8 | 8.6 | 1.8x |
| 10mb | clustered | intersects_count_small | 23/26 | 0/26 | 1.1 MB | 10.0 MB | 6.3 | 18.4 | 2.9x |
| 10mb | clustered | aggregate_by_category_small | 23/26 | 0/26 | 1.1 MB | 10.0 MB | 5.1 | 15.9 | 3.1x |
| 10mb | clustered | spatial_join_small | 46/52 | 0/52 | 2.3 MB | 20.0 MB | 10.6 | 34.5 | 3.2x |
| 10mb | clustered | return_rows_small(id,geometry) | 23/26 | 0/26 | 1.1 MB | 10.0 MB | 6.1 | 38.3 | 6.3x |
| 10mb | uniform | bbox_count_small(0.6%) | 19/26 | 0/26 | 2.7 MB | 10.0 MB | 10.3 | 12.9 | 1.2x |
| 10mb | uniform | bbox_count_large(25%) | 14/26 | 0/26 | 4.6 MB | 10.0 MB | 10.1 | 14.3 | 1.4x |
| 10mb | uniform | attr_filter_fullscan | 0/26 | 0/26 | 9.9 MB | 10.0 MB | 7.3 | 8.8 | 1.2x |
| 10mb | uniform | intersects_count_small | 19/26 | 0/26 | 2.7 MB | 10.0 MB | 10.5 | 21.2 | 2.0x |
| 10mb | uniform | aggregate_by_category_small | 19/26 | 0/26 | 2.7 MB | 10.0 MB | 11.1 | 13.5 | 1.2x |
| 10mb | uniform | spatial_join_small | 45/52 | 0/52 | 2.7 MB | 20.1 MB | 14.8 | 42.5 | 2.9x |
| 10mb | uniform | return_rows_small(id,geometry) | 19/26 | 0/26 | 2.7 MB | 10.0 MB | 13.6 | 49.2 | 3.6x |
| 100mb | clustered | bbox_count_small(0.6%) | 61/65 | 0/65 | 6.1 MB | 101.1 MB | 11.6 | 44.0 | 3.8x |
| 100mb | clustered | bbox_count_large(25%) | 48/65 | 0/65 | 25.8 MB | 101.1 MB | 20.3 | 44.7 | 2.2x |
| 100mb | clustered | attr_filter_fullscan | 0/65 | 0/65 | 98.3 MB | 101.1 MB | 18.1 | 14.7 | 0.8x |
| 100mb | clustered | intersects_count_small | 61/65 | 0/65 | 6.1 MB | 101.1 MB | 16.7 | 125.1 | 7.5x |
| 100mb | clustered | aggregate_by_category_small | 61/65 | 0/65 | 6.1 MB | 101.1 MB | 11.9 | 48.6 | 4.1x |
| 100mb | clustered | spatial_join_small | 123/130 | 0/130 | 10.7 MB | 202.2 MB | 30.6 | 204.5 | 6.7x |
| 100mb | clustered | return_rows_small(id,geometry) | 61/65 | 0/65 | 6.1 MB | 101.1 MB | 22.6 | 368.3 | 16.3x |
| 100mb | uniform | bbox_count_small(0.6%) | 55/65 | 0/65 | 15.3 MB | 101.4 MB | 14.7 | 43.9 | 3.0x |
| 100mb | uniform | bbox_count_large(25%) | 36/65 | 0/65 | 44.3 MB | 101.4 MB | 25.1 | 46.7 | 1.9x |
| 100mb | uniform | attr_filter_fullscan | 0/65 | 0/65 | 99.1 MB | 101.4 MB | 18.4 | 16.8 | 0.9x |
| 100mb | uniform | intersects_count_small | 55/65 | 0/65 | 15.3 MB | 101.4 MB | 22.1 | 136.1 | 6.2x |
| 100mb | uniform | aggregate_by_category_small | 55/65 | 0/65 | 15.3 MB | 101.4 MB | 16.6 | 44.6 | 2.7x |
| 100mb | uniform | spatial_join_small | 123/130 | 0/130 | 10.7 MB | 202.7 MB | 30.6 | 265.0 | 8.6x |
| 100mb | uniform | return_rows_small(id,geometry) | 55/65 | 0/65 | 15.3 MB | 101.4 MB | 44.2 | 405.2 | 9.2x |
| 1gb | clustered | bbox_count_small(0.6%) | 126/130 | 0/130 | 30.1 MB | 1004.9 MB | 18.7 | 437.9 | 23.5x |
| 1gb | clustered | bbox_count_large(25%) | 97/130 | 0/130 | 246.4 MB | 1004.9 MB | 87.0 | 452.5 | 5.2x |
| 1gb | clustered | attr_filter_fullscan | 0/130 | 0/130 | 968.5 MB | 1004.9 MB | 34.1 | 72.2 | 2.1x |
| 1gb | clustered | intersects_count_small | 126/130 | 0/130 | 30.1 MB | 1004.9 MB | 68.2 | 1143.5 | 16.8x |
| 1gb | clustered | aggregate_by_category_small | 126/130 | 0/130 | 30.1 MB | 1004.9 MB | 19.5 | 452.4 | 23.2x |
| 1gb | clustered | spatial_join_small | 253/260 | 0/260 | 52.7 MB | 2009.9 MB | 50.3 | 2068.1 | 41.1x |
| 1gb | clustered | return_rows_small(id,geometry) | 126/130 | 0/130 | 30.1 MB | 1004.9 MB | 48.0 | 3730.9 | 77.7x |
| 1gb | uniform | bbox_count_small(0.6%) | 118/130 | 0/130 | 90.3 MB | 1007.2 MB | 63.7 | 448.4 | 7.0x |
| 1gb | uniform | bbox_count_large(25%) | 81/130 | 0/130 | 368.7 MB | 1007.2 MB | 180.3 | 469.3 | 2.6x |
| 1gb | uniform | attr_filter_fullscan | 0/130 | 0/130 | 977.6 MB | 1007.2 MB | 92.7 | 73.8 | 0.8x |
| 1gb | uniform | intersects_count_small | 118/130 | 0/130 | 90.3 MB | 1007.2 MB | 88.5 | 1287.1 | 14.5x |
| 1gb | uniform | aggregate_by_category_small | 118/130 | 0/130 | 90.3 MB | 1007.2 MB | 54.5 | 453.6 | 8.3x |
| 1gb | uniform | spatial_join_small | 251/260 | 0/260 | 67.7 MB | 2014.5 MB | 107.1 | 2392.7 | 22.3x |
| 1gb | uniform | return_rows_small(id,geometry) | 118/130 | 0/130 | 90.3 MB | 1007.2 MB | 218.3 | 4111.7 | 18.8x |


Reading: pruning gains grow with size (1 GB, 0.6 % viewport: 23.5× on the clustered set, 7.0× on the uniform set; return-rows 77.7× / 18.8×; intersects 16.8× / 14.5×). For the attribute-only query (no spatial predicate) the spatial/shuffled ratio is 0.8×–2× and is unexplained noise or a compression/decoding effect — it is **not** pruning. Numbers differ from the MVP report's run (different run, noisy small timings); no conclusion depends on a single small timing.

## 10. Gateway Tests

In-process router (no sockets), real runtime + DuckDB + benchmark data: `crates/geo-gateway/tests/api.rs`, 7 tests (8 scenarios) — all pass. Valid query (schema keys, GeoJSON, provenance); malformed bodies (invalid JSON, missing content-type, wrong shape → 4xx); invalid IR and unsupported IR version (400 `bad_ir`), bad source id, unknown kind, CRS mismatch; execution error (unknown filter column → 422 `invalid_plan`) and recovery; unauthorized sources (§6); execution timeout (504) with slot and runtime recovery; client-disconnect cancellation. The gateway was split into `lib.rs` (router, config, handlers) and a thin `main.rs`. Wkb unit tests: 3.

## 11. Browser Smoke Tests

Playwright (`@playwright/test`) driving the **installed Chrome** (no browser download), starting the real gateway; one worker. `web/e2e/smoke.spec.ts` (needs `npm run build`, benchmark data). The hook `window.__gf` is exposed only with `?e2e=1`, used to zoom to a real feature and click it on the canvas.

| check | result |
|---|---|
| app loads, map container + WebGL canvas rendered | **PASS** |
| dataset listed with CRS metadata | **PASS** |
| query/run action works (auto-run on select, and **Run** after zoom) | **PASS** |
| Results tab shows header `category` and ≥ 1 row | **PASS** |
| Execution Plan tab shows steps (metadata, compile, planner decisions) and provenance badges (`Bytes read` = Unknown/Unavailable, `Row groups skipped` = Estimated, `Rows returned` = engine-observed) | **PASS** |
| click a feature on the map → Inspector shows Properties, geometry type, category, CRS, Lineage | **PASS** |
| no uncaught page errors | **PASS** |

Limitations: one flow, one browser, no visual regression, headless only; the click goes through a real mouse event on the canvas but the zoom is set programmatically.

## 12. DuckDB Integration

- **Bundled (default)**: compile DuckDB 1.5.6 from source. Clean builds on this machine at `CARGO_BUILD_JOBS=3`: workspace test build ≈ 10 min, release `geo-bench` 7 m 35 s (earlier at jobs=8: debug 169 s, release 298 s); incremental ≈ 10 s. This round hit two real problems: **target/ reached 59.6 GB and filled the disk (12 MB free)**, and with debug info the test binaries exceeded the **MSVC PDB limit (LNK1140)**. Fix: `[profile.dev] debug = 0`; `cargo clean` was run (59.6 GiB removed). After `cargo clean` the rebuilt `target/` (debug tests + release bench) is far smaller because dev has no debug info.
- **Dynamic / prebuilt**: **evaluated and works for the engine crate**. `geo-engine-duckdb` now has a `bundled` feature (default). With `--no-default-features` and `DUCKDB_DOWNLOAD_LIB=1` the `libduckdb-sys` build script downloads the official prebuilt `libduckdb` for the pinned version (1.5.6) and links it dynamically. Measured on this machine (jobs=3, separate target dir): clean build of the adapter crate **54 s** (vs ≈10 min for the bundled workspace build), `target/` **747 MB** (vs tens of GB), `duckdb.dll` 36.7 MB must sit next to the executable / on PATH. A probe (`cargo run --example probe`) loaded the adapter, reported `duckdb v1.5.6`, `spatial=true` and all ten ops. **Not done:** feature forwarding through `geo-runtime` / `geo-gateway` / `geo-bench`, so the full test suite and benchmarks were run only with the bundled build; the prebuilt build was not run against the test suite, and the build script downloads a binary at build time (supply-chain / offline considerations). It is therefore a viable development option, not yet a validated one.
- **Spatial extension**: still loaded at runtime (`INSTALL`/`LOAD`, ~66 s first time, ~/.duckdb cache); the adapter advertises reduced capabilities when it is missing. Version pinning: the crate pins DuckDB 1.5.6 (`libduckdb-sys 1.10506`); the extension must match (`v1.5.6`).
- **WKB bridge**: unchanged and still required — native GEOMETRY → Arrow export fails in the Rust binding, so the adapter uses `ST_AsWKB` and re-attaches GeoArrow metadata/CRS from the IR (now handling the four `Crs` variants).
- Strategy (ADR-006 updated): development = prebuilt libduckdb if it works for you, otherwise bundled; CI/release = bundled with pinned versions (reproducible).

## 13. Architecture Debt

- **P1 — IR is a tree, not a DAG.** Accepted for the MVP. Current: tree. Target: `PlanGraph { nodes, edges, root }` (RFC-001). Not implemented; not claimed.
- **P1 — DuckDB reads Parquet directly** (accepted temporarily). Real pipeline is documented in `docs/pipeline.md`: Spatial IR → planner → DuckDB → DuckDB reads Parquet → WKB → GeoFabric re-attaches GeoArrow/CRS → Arrow. **Not** zero-copy; GeoFabric's Arrow reader does not control execution; row-group pruning is estimated, not observed.
- **P1 — only one real executor**; replaceability shown only with mocks.
- P2 — planner reads Parquet footers (cheap) and is not covered by the timeout.

## 14. Security / Safety

Authorization boundary with opaque denials, DuckDB confined to the authorized files, SQL never accepted, WKT charset restricted, result/body/concurrency caps, execution timeout + cancellation, gateway bound to 127.0.0.1. **Still missing:** authentication, per-user authorization policy (the guard is root-based only), CPU/time isolation beyond the interrupt, dependency audit (`cargo audit` not run), remote sources (refused).

## 15. ADR Status

| ADR | status | reason |
|---|---|---|
| 001 Rust core | Proposed | works natively; WASM argument unproven; build/disk/PDB friction |
| 002 Arrow/GeoArrow internal | Proposed | WKB + GeoArrow metadata works, but not zero-copy, no native GeoArrow encoding, copies unmeasured |
| 003 GeoParquet first | Proposed | strong evidence incl. validated 1 GB runs and CRS semantics, but synthetic data only (no real files / remote storage) |
| 004 DuckDB first engine | Proposed | works, timeout/sandbox verified; no second real engine; no trustworthy scan metrics |
| 005 MapLibre rendering | Proposed | smoke test passes; GeoJSON boundary is a stop-gap, delivery format unevaluated |
| 006 DuckDB integration | Proposed | measured on one machine; prebuilt evaluation recorded; CI strategy not exercised |

Nothing was promoted: existing code was not treated as evidence. Each ADR now has an "Evidence review" section.

## 16. Spec Deviations (cumulative, still true)

1. Flat `crates/` instead of `engines/`, `formats/`, `gateway/ogc-*`, `sdk/`.
2. IR is a tree (accepted, P1 debt).
3. Execution reads Parquet inside DuckDB (accepted temporarily, P1 debt); scan numbers are estimates.
4. UI "SQL / Ask" accepts logical IR / templates, not SQL; no AI.
5. Planner v0: no cost model; thresholds are placeholders.
6. Process: Codex was used once (geo-types), its output violated the spec and was discarded; **no Codex tasks were used in this round** — the hardening work was done by Claude directly, so the lead/implementer split in the brief was again not followed.
7. `OgcCrs84` is deliberately not equal to `Epsg(4326)`; there is no transform node, so data with an absent CRS can only be queried in CRS84.
8. Buffer execution remains untested (no projected dataset; unchanged from the MVP report).

## 17. Remaining Technical Debt

- **P0:** none open from the release gate. (Authentication is required before the gateway is ever exposed beyond localhost — treat as P0 for any such deployment.)
- **P1:** IR tree vs DAG; DuckDB direct Parquet path; single real executor; real-data validation (needs approval to download); actual I/O measurement (needs OS counters or an engine patch); CI wiring of `arch_check`, tests, oracle and the e2e smoke test.
- **P2:** Buffer execution + `Transform` node; second engine; cost model; DuckDB connection per query (extension reload ~100 ms in debug); e2e covers one flow only; `cargo audit`.
- **P3:** light-theme/accessibility review of the UI; table cap 1 000 rows; no basemap.

## 18. Recommendation

**APPROVE CORE MVP**, with this exact meaning: every item of the release gate holds — all tests pass (66 + browser smoke), spatial join verified by exact pairs, timeout and cancellation really interrupt DuckDB, authorization precedes any metadata I/O, 100 MB and 1 GB results are verified against an independent oracle (84/84), metrics carry provenance, the misleading bytes-read number is removed, GeoParquet CRS semantics follow the spec, gateway tests and browser smoke pass.

What this approval does **not** cover: performance on real-world data, actual I/O volume, engine replaceability beyond a mock, WASM, production security (no authentication), Buffer execution. These are open and listed above; none of them should be inferred from this round.

## 19. Files Changed (important)

`crates/geo-types/src/{lib,access}.rs`, `crates/geo-ir/{src/validate.rs,tests/ir.rs}`, `crates/geo-executor/src/lib.rs`, `crates/geo-source-geoparquet/{src/lib,src/analysis}.rs`, `crates/geo-engine-duckdb/{Cargo.toml,src/lib,src/sql}.rs`, `crates/geo-planner`, `crates/geo-runtime/{src/lib.rs,tests/join.rs,tests/timeout.rs,tests/vertical.rs}`, `crates/geo-gateway/{src/lib.rs,src/main.rs,tests/api.rs}`, `crates/geo-bench/src/main.rs`, `benchmarks/oracle.py`, `benchmarks/results/{expected-*,bench-h-*,summary-hardening-*}`, `web/src/{main.ts,style.css}`, `web/e2e/smoke.spec.ts`, `web/playwright.config.ts`, `tools/arch_check.py`, `Cargo.toml` (`debug = 0`), `.vscode/settings.json`, RFC-001/002, ADR-001..006, `docs/pipeline.md`, `README.md`.
