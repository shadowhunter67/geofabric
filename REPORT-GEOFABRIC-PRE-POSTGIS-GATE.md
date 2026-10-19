# REPORT — GeoFabric pre-PostGIS gate (2026-10-19)

Written for: the Principal Architect reviewing whether PostGIS may start. Nothing of PostGIS was started.

## Executive Summary

Four work groups were done; the gate is **not fully closed**.

| Group | Result |
|---|---|
| Browser cost model | Component model fitted on the calibration set only, frozen, implemented in Rust. Fresh held-out (seed 20261019, new data draw, 164 cases, run once): **median 1.19× (gate 1.25×), P90 2.28× (gate 3×)**, P95 2.54×, max 10.2×. Planner accuracy 96.3 %, regret P90 1.00. **But**: LOCAL/FAST_LAN are under-predicted by ~1.75×, and no second physical machine was available. |
| Metric distance | Done. UTM zone from the query area, refusal rules, explicit `Transform`, `Distance{value,unit}`. Independent oracle (pyproj + shapely): **27,624 = 27,624, identical id sets**. |
| `SpatialSemiJoin` | Done in IR 0.3 (validation, serialisation, schema, explain, DuckDB EXISTS lowering, 6 correctness cases). The browser engine does **not** execute it: it declares `SpatialSemiJoin` and `Transform` unsupported through the canonical lowering path (golden-tested), so Demo B is server-only by design. Demo B migrated; oracle-checked. |
| Representative rendering | Done. Deterministic md5 thinning, UI message vi/en, counts never sampled; four-cluster test; no performance regression in release builds. |

Things found and fixed on the way (each with evidence below): a harness artifact that inflated every earlier browser timing; an estimator blind spot for polygons larger than the window; 8 cross-engine digest mismatches (non-deterministic `count_by` tie-break); a browser-model failure on ≥ 1 GB files discovered *after* the held-out (policy R5c); two test races found by the flake loop.

**PostGIS readiness: KEEP HARDENING PLANNER** (reasons in the last section).

## Browser Cost Model

`crates/geo-planner/src/browser_model.rs`. Browser time = plan round trip + engine start (cold only) + opening sources + range requests and their bytes + decode/compute + result materialisation. All inputs are known before execution: row groups kept, bytes, covering bytes, LIMIT-bound or not, geometry/attribute bytes per row (and, for small sources, the exact geometry bytes of the rows in the window), network profile, warm/cold. A throttled link costs a request `0.97·RTT + 25.7 ms` (the emulated link adds ~25 ms per request beyond the RTT).

## Cost Decomposition

Fitted per component by NNLS (`benchmarks/fit_browser_model.py`) from the phases the engine measures itself:

| part | formula (ms) |
|---|---|
| plan round trip | 7.0 + 1.18·RTT + 24.8·[throttled] |
| cold start | 388 (engine CPU) + 0.95·wasm(36.7 MB)/link + 1.35·RTT + 15.5·[throttled] + 165 (first-query warm-up) |
| whole-file open | 8.3 + 1.42·RTT + 1.02·file/link + 15.5·[throttled] |
| whole-file first query | 4.2 + 0.54·kept MB + 6.1·result-geometry MB |
| range first query | 5.9 + n·(2.8 + 0.97·RTT + 25.7·[throttled]) + 1.005·bytes/link |
| result | 0.79 + 3.2/krow + 6.0/geometry-MB + 18.6/attribute-MB |
| range requests, bytes | fitted from planner-known features: aggregates 3.4 + 2.87·kept row groups requests, 1.16× kept bytes; typed files 4.7·kept requests; LIMIT-bound features 14 requests / ≤ 6.4 MB; free features 4.1 + 4.2·kept requests, 1.76× kept bytes |

Harness artifact (disclosed): before this phase the harness returned the whole result through Playwright's `evaluate`, so serialisation was counted as engine time. Every earlier browser timing, including the browser column of held-out v5 (median 1.37×), was inflated and is **not comparable**. Fixed; summaries only are returned now.

## Calibration

159 measured runs (`benchmarks/results/bcal-*.json`; seed 20261020; synthetic draws `_s2`, `_s3`, 1 GB shuffled files, and the four Overture pack layers; 5 networks by Chrome CDP throttling; 30 % cold). Fit on these only.

| | median | P90 | P95 | max |
|---|---|---|---|---|
| in-sample (Rust planner = Python fit) | 1.07× | 1.51× | 1.82× | 6.4× |
| with measured instead of predicted requests/bytes | 1.04× | 1.30× | | |
| leave-one-dataset-family-out | 1.05× | 1.46× | | 11.6× |

## Fresh Held-out

Frozen model, frozen estimator (row index included), frozen calibration, `--no-learn`. New seed 20261019 (20261018 not reused), new data draw `_s5` of the synthetic generator (12 files, sizes 10/30/100 MB, spatial and shuffled; no 1 GB twin exists so none was used), 164 cases: LOCAL 44, FAST_LAN 28, GOOD_BROADBAND 32, MOBILE 32, HIGH_LATENCY 28; ≈ 30 % cold (46 cold by plan, 49 by the engine's own state at run time); features 102, count 24, count_by 38. The four Overture pack layers are the same files as in calibration (new windows only): the draw is fresh for the synthetic data, not for the real layers. Gateway ports checked free and profile verified per run. Run once, no code or constant changed between calibration and the run.

## Model Error

Browser time model, fresh held-out (`benchmarks/results/bheld-*.json`, `bheld-table.json`):

| slice | n | median | P90 | P95 | max |
|---|---|---|---|---|---|
| **all** | 164 | **1.19×** | **2.28×** | 2.54× | 10.2× |
| LOCAL | 44 | 1.74 | 2.62 | 3.21 | 3.78 |
| FAST_LAN | 28 | 1.79 | 2.44 | 3.04 | 3.48 |
| GOOD_BROADBAND | 32 | 1.15 | 1.68 | 1.97 | 2.41 |
| MOBILE | 32 | 1.04 | 1.34 | 1.41 | 2.44 |
| HIGH_LATENCY | 28 | 1.01 | 1.28 | 1.45 | 10.2 |
| whole-file mode | 111 | 1.15 | 2.30 | 2.50 | 3.76 |
| range mode | 53 | 1.41 | 2.06 | 2.81 | 10.2 |
| warm | 115 | 1.17 | 2.39 | 2.84 | 10.2 |
| cold | 49 | 1.22 | 1.80 | 2.01 | 2.78 |
| features / count / count_by | 92/40/32 | 1.41 / 1.13 / 1.16 | 2.42 / 1.88 / 2.10 | | 10.2 / 2.5 / 2.7 |

Honest reading: both overall gates are met, but on LOCAL and FAST_LAN the model under-predicts by a median ~1.75× (measured/predicted: 1.74 and 1.79; 1.0–1.13 on the other three). Absolute errors are tens of ms (e.g. 85 vs 36 ms). In the same run the plan round trip alone (same operation) took 11 ms against 7 ms during calibration, so part may be machine state; **not proven, not refitted**. The max (10.2×) is one HIGH_LATENCY range case (`uniform_100mb_shuffled_s5`, 1,500 features, measured 54.8 s, predicted 5.4 s): shuffled 100 MB file, many row groups, the request-count prediction was too low.

Cardinality estimator on the same run: median 1.00×, P90 1.19×, P95 2.0×, max 3.14×, interval coverage 96 %, 0 underestimates beyond 5×. Admin boundaries (exact row index): 1.00×.

## Planner Accuracy

164 cases, both engines measured in all: **96.3 %** correct (always-server: 82.3 %; always-browser regret P90 83×). Excluding 4 near-ties: 96.9 %. Browser fastest in 29 cases, chosen in 27. By profile: FAST_LAN 92.9 %, GOOD_BROADBAND 96.9 %, HIGH_LATENCY 96.4 %, LOCAL 95.5 %, MOBILE 100 %; warm 94.8 %, cold 100 %. Confidence ≥ 0.80: 99 % accurate (130 cases); 0.65–0.80: 90 % (29); 0.50–0.65: 60 % (5). Unsafe browser choices: 0; browser crashes 0.

## Planner Regret

Regret = selected time / fastest time. P50 1.00, P90 1.00, P95 1.00, **max 4.63**. Six wrong choices: 4.63 (25-row admin query, FAST_LAN; server 2,327 ms vs browser 503 ms), 1.82, 1.53, 1.56, 1.35 and a near-tie 1.03. Three of the six are the heavy admin-polygon layer, where the server was slow, where the *server* time model is weak (server model median 1.46×, max 25×: not part of this gate, listed under risks). Absolute regret P90 = 0 ms, max 1.8 s.

## Independent Device Evidence

**Limitation first: no second physical machine was available.** What was done (`benchmarks/run_devicecheck.mjs`, in-page matrix `?devicecheck=1`, 8 cases incl. one the planner must keep on the server):

| browser (engine) | machine | result |
|---|---|---|
| Chrome 155 (Blink) | this machine | 7/7 runnable cases: browser digest = server digest; unsafe candidate stayed on the server |
| Edge 155 (Blink) | this machine | same |
| Playwright WebKit (JavaScriptCore/WebKit, Windows build) | this machine | same: 7/7 digests equal |

Same-machine Edge is **not** counted as independent evidence (per the acceptance criteria); WebKit is a different engine and counts as *partial* evidence (engine-independence of the correctness path, not of timing, and not of memory limits). Firefox was not installed (≈ 90 MB download, not done without approval). For a second laptop: start the gateway with `--devicecheck-dir <dir>`, open `http://<host>:8787/?devicecheck=1` on that device, wait for "done"; the report is saved by the server (`benchmarks/results/devicecheck/`).

## Metric Spatial Semantics

ADR-011. A distance with a linear unit is only evaluated in the plane of a local metric CRS; degrees are never converted to metres. Validator refusals: `angular_distance_predicate`, `not_metric_crs` (geographic, Web Mercator, unknown). `ST_DWithin(l, r, d)` planar, inclusive, null/empty never match. UI says "Trong bán kính 200 m" / "within 200 m" only where the metadata (`metric_semantics`) says the computation was metric.

## CRS Strategy

`geo_types::local_metric_crs`: UTM zone (326xx/327xx) from the centroid of the query area grown by the distance; refuses (HTTP 422 `metric_crs_unavailable`, UI asks to zoom in) when the area is not in a known geographic CRS, outside 80°S–84°N, or the worst scale error exceeds 0.5 % (`k = 0.9996(1+(Δλcosφ)²/2)`, Δλ·cosφ ≤ 5.9°). `Transform` is an explicit plan node (`always_xy := true`); the original geometry stays in its CRS, so results are drawn in lon/lat. Error: 0.02–0.03 % in the demo area. Not covered: national grids (VN-2000), geodesic distance for areas no zone covers.

## Units

`Distance { value, unit }` (meter, kilometer, foot, degree) in the IR; a different unit is a different plan id; no silent normalisation. A degree distance is a degree distance and warns.

## SpatialSemiJoin

IR 0.3: `SpatialSemiJoin { left, right, left_column, right_column, predicate }`, each left row at most once, output schema = left schema, both columns geometry in the same known CRS. DuckDB lowering `WHERE EXISTS (… ST_DWithin …)` (0.06 s; the buffer-and-join formulation took 23 s and gave 27,620). Browser lowering: the canonical path refuses `SpatialSemiJoin` and `Transform` with an `unsupported` error (golden fixtures `semi_join_is_unsupported_in_the_browser`, `transform_is_unsupported_in_the_browser`); the browser engine is single-source, as it already was for `SpatialJoin`. The brief's "browser lowering" is therefore met as an explicit, tested refusal, not as an execution path — a limitation. Six correctness cases (`crates/geo-runtime/tests/semi_join.rs`, `crates/geo-ir/tests/semi_join.rs`): no match, one match, many matches (no duplicates), boundary inclusive, null geometry, empty geometry; plus topological behaviour and four points placed by a geodesic computation at 199 m / 201 m. 0.2 plans are read unchanged.

## Demo B Migration

Demo B ("buildings within 200 m of main roads") now runs `SpatialSemiJoin` in a UTM plane; `near` returns the buildings once, `near_count` the distinct count. The pre-semantics demo said 27,362 (degrees as metres, a pair table).

## Independent Oracle

`benchmarks/oracle_near.py` — pyarrow + shapely 2 STRtree `dwithin` + pyproj; no DuckDB, no GeoFabric code in the computation. 4 km window, 200 m, 41,150 buildings and 1,279 main-road segments in the window: oracle **27,624**, GeoFabric **27,624**, id sets identical (same sha256), UTM 48N. Against true WGS 84 geodesic distances: 3 of 1,955 pairs within 5 % of the limit classify differently (0.15 %). 1.6 s end to end.

## Representative Rendering

ADR-012. When a row-returning query hits the render budget, the result is thinned by `md5_number_lower(CAST(id AS VARCHAR)) % m = 0` with m a power of two (deterministic, nested, stable while panning, identical in both engines — checked by digest). The first capped answer is only kept if everything fits. Four-corner cluster test (`crates/geo-gateway/tests/templates.rs`): the file-order cut shows one cluster, the thinned answer all four. Real buildings: `web/e2e/sampling.spec.ts` (3 tests, quadrant shares within 4 points of the truth, the old cut ≥ 2× worse; deterministic digest; server = browser sample).

## Rendering Cap Semantics

Counts, aggregates and joins never see sampling (an aggregate with `sample_modulus` is refused with HTTP 400). UI: "Hiển thị mẫu đại diện 12.234 / 41.150 đối tượng" / "Showing a representative sample of 12,234 / 41,150 features"; the shown number is between budget/2 and budget (power-of-two steps). An exact-LIMIT mode (first N rows) remains as a checkbox. A source without an `id` column falls back to the exact-limit message.

## Real-data Workflows

`web/e2e/data-pack.spec.ts` (workflows A–E on the Overture pack: catalog, layers, inspector, demos): **15/15 on Chrome and 15/15 on Edge**. Workflow B uses the new semantics; workflow C says ward/commune (phường/xã), not district.

## Performance

Release gateway, real buildings (15.4 M rows), time from request to drawn (`pack-perf.spec.ts`; Playwright runs the *debug* gateway unless `GATEWAY_BIN` is set — the first comparison made that mistake and was discarded):

| | before (file-order cut) | after (representative) |
|---|---|---|
| z16 small, 783 features | 416 ms | 332 ms |
| z14 medium | 2,238 ms (20,000 rows) | **1,776 ms** (12,234 sampled of the viewport) |
| z12 large | 2,255 ms (20,000) | 2,009 ms (13,259) |
| z14 capped query / count / thinned query (server) | 0.6 s | 0.6 s / 0.13 s / 0.5–0.7 s |
| Demo B (4 km, 200 m) | pair table, degrees | 1.6 s end to end, 27,624 distinct |

No performance regression in release; the cost of sampling (two extra requests) is paid back by drawing fewer features.

## Regression

| suite | result |
|---|---|
| Rust workspace (`cargo test --workspace`) | **189 passed, 0 failed** |
| TypeScript | typecheck clean, **76 passed** (`node --test`) |
| arch_check | OK, no dependency-boundary violations |
| Playwright Chrome (combined suite, 45 tests) | 14 iterations, 0 failures (after the 2 test fixes below) |
| Playwright Edge | 6 iterations, 0 failures |
| data-pack (workflows A–E) | Chrome 15/15, Edge 15/15 |
| device matrix | Chrome, Edge, WebKit — see above |
| oracle | Demo B identical ids |
| held-out digests | 164 runs; 72 digest-checked: 64 match, **8 mismatched** → cause fixed, 8/8 re-run match |

Cross-engine digest bug: all 8 mismatches were `count_by` cut at 50 groups with ties on the count; the engines ordered ties differently. Both templates now sort `count DESC, <group key> ASC`. The 8 cases were re-run on the fixed build (`btie-*.json`): 8/8 identical digests. This is a re-run of the failing cases, not a new held-out.

Tests that encoded the old planner (and were wrong for the new evidence) were updated: IR version assertions (0.3), cold start priced at ~0.6 s (was a fixed 1.4 s), the 100 MB case (browser range mode is comparable, not 2× slower), the 1 GB case (now asserts policy R5c), the real Overture case, and `engines.spec.ts` "auto mode" (the browser is now often the default on small files over a fast link).

## Flake Investigation

`benchmarks/flake_loop.py` / `flake_run_all.sh`: combined suite (smoke, explorer, engines, planner, map, sampling; 45 tests) on the release gateway, port 8798 checked free, gateway PID recorded, JSON report per iteration. The first iterations failed:

| iteration (old tests) | test | symptom | cause |
|---|---|---|---|
| probe 1 | `engines`: auto mode shows the server engine | selected `duckdb-wasm` | test encoded the old default; planner now picks the browser on small files over LOCAL (measured faster: 66 vs 129 ms) → test rewritten |
| 1, 2 (+ 5 of 12 solo repeats) | `map`: clicking a polygon selects exactly that feature | wrong feature selected / 0 highlighted / inspector not shown | the test picked its target 7,570 / 6,031 / **958** polygons into a map still drawing; no wait for idle/stable count → now waits for idle and a stable count (20/20 solo pass) |
| 2, 5, 9, 10, 11, 12, 13 (≈ 25 % solo) | `engines`: browser fails → fallback shown | `engine-fallback` never appears | the page's runtime probe also runs queries on the browser engine; the injected failure sometimes hit the probe, the planner (correctly) marked the browser unfit and never tried it → test waits for the probe first (16/16 solo pass) |

Both are test races, not product defects (the product behaviour in each case was correct). Evidence of the failing runs is kept (`flake-before-fix-chrome.json`, `flake-pass2-partial-chrome.json`). **Final loop with both fixes: 20 combined iterations (14 Chrome, 6 Edge), 900 test executions, 0 failures** (`flake-chrome.json`, `flake-msedge.json`, `flake-run-all.log`; per iteration: start time, port 8798, gateway PID, browser, failures; the gateway is shared state across the specs of one iteration and is restarted per iteration). Not covered: iterations on a second physical machine; Firefox/WebKit are not in the Playwright suite. No other flake was seen in 20 iterations + 2 data-pack runs + 3 device runs; absence of failures in 20 runs bounds the per-iteration flake rate below ≈ 15 % at 95 % confidence.

## ADR/RFC Status

- **ADR-010** (planner confidence/health/network/model): addendum round III written; **remains Proposed**. Gates met on aggregate (median/P90), not accepted because independent device evidence is same-machine and LOCAL/FAST_LAN bias is unexplained.
- **ADR-011** (metric semantics): Accepted; oracle evidence added.
- **ADR-012** (representative rendering): Accepted; evidence added.
- **RFC-001** (Spatial IR): updated with `Transform`, `SpatialSemiJoin`, `WithinDistance`, `HashBucket`, IR 0.3.
- **ADR-008** (multi-engine placement): remains Proposed (evidence not strong enough: one machine).
- ADR-009: unchanged (Accepted for Chromium, one machine).

## Remaining Risks

1. **Outside the calibrated range the model is wrong.** After the held-out, files beyond 100 MB were measured on purpose (1.45 GB buildings, two 1 GB synthetic files, LOCAL, 48 runs): the browser takes 1.5–3× the server time warm (up to 10× cold) where the model predicted a win (e.g. 69 ms predicted vs 190–320 ms measured). Policy R5c (no automatic browser choice above 128 MiB) is a safety rule decided from evidence gathered *after* the held-out; it changes no held-out decision (all held-out files ≤ 100 MB) and is itself untested on a fresh set.
2. LOCAL/FAST_LAN under-prediction (~1.75×) unexplained.
3. Tail: one HIGH_LATENCY shuffled 100 MB range case predicted 10× too fast (request count).
4. Server time model is weak on heavy-polygon layers (median 1.46×, max 25×) and drives the worst regret (4.6×).
5. No second physical machine; Firefox not tested; WebKit evidence is correctness only.
6. The row index exists only for sources ≤ 100k rows; larger polygon layers still use the centre histogram (underestimates objects larger than the window).
7. Metric distance: UTM only (≤ 0.5 % scale error, refuses beyond); no national grid, no geodesic fallback. Distances are ground distances up to the scale error, not survey grade.
8. Held-out reuses the four Overture pack files from calibration (new windows only).
9. Codex was used only in read-only delivery mode (its sandbox cannot write files); all code here was written or reviewed by Claude.

## PostGIS Readiness

**KEEP HARDENING PLANNER.** The metric semantics, `SpatialSemiJoin` and rendering work are in a state PostGIS could build on (REWORK SPATIAL SEMANTICS is not needed: the oracle agrees exactly and the IR carries units/CRS/transform explicitly). The planner is not ready: the browser model passes the aggregate gates but has an unexplained 1.75× bias on the fast profiles, its extrapolation beyond 100 MB failed and is covered only by a conservative policy, and there is no independent physical device. Suggested next steps before PostGIS: (a) run the device matrix on a second physical laptop and one Firefox; (b) a second fresh held-out (new seed, new draw) that includes ≥ 1 GB files and the R5c policy; (c) explain or remove the LOCAL/FAST_LAN bias (repeat LOCAL calibration vs held-out on a quiet machine, compare plan round trip); (d) model the server side for heavy polygons.
