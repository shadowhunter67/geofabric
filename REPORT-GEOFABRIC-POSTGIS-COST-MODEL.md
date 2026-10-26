# GeoFabric — PostGIS cost model, semi-join risk model, held-out planner validation, class-scoped R5d

Repository <https://github.com/shadowhunter67/geofabric> · Live demo <https://shadowhunter67.github.io/geofabric/>. Base `f44c6ef`; this phase `a5dd14b` … `801d776` and the report commit. 2026-10-23. Decision record: `adr/ADR-015-postgis-cost-model.md`. No engine added, no UI redesign, no IR change, no change to data hosting or the shared lowering.

> **SUPERSEDED IN PART (2026-10-26) — read "Follow-up" first.** The timings, coefficients and class table in this report were measured with a PostGIS connector that added up to ~15 ms to every query. After the fix everything was re-measured and the model refit (`postgis-cost-v2`); the validated classes are now `indexed_bbox`, `aggregate` and `semijoin_intersects`, and the DuckDB "abort" described below turned out to be the machine's RAM killer. Correctness results (0 mismatches, Demo B, wide semi-join) are unaffected. Details: ADR-015 addendum.

## Executive Summary

* **PostGIS is now chosen automatically, on measured evidence, for four workload classes**; everywhere else policy R5d still applies. R5d is **scoped by validated class** (option B), not removed (option C).
* **Validated classes** (confirmation set B, 260 fresh cases, model coefficients as shipped): selective indexed windows, intersects / projected columns, aggregates up to 300,000 matching rows, and the admin-style intersects semi-join. Inside them the planner has **P90 regret 1.00** (max 1.86, max penalty 323 ms) on B and 1.00 on A.
* **Not validated, R5d stays**: broad scans (> 300,000 rows read) and both **metric semi-join** classes. Plan-time features cannot predict the metric semi-join time better than a median ×2–3 (P90 ×5–7); the faster engine flips with right-side density and distance. A **hard risk policy** refuses automatic PostGIS selection above 50,000 right-side probes (the wide 189,772-id case stays on DuckDB).
* **Against the old behaviour** (always DuckDB under blanket R5d), on B: P90 regret **14.7 → 2.5**, mean absolute penalty **3.6 s → 0.2 s**; admin semi-joins that cost DuckDB 4–60 s now run in 0.05–0.3 s.
* **The primary gate "P90 regret ≤ 1.5" is MET on the natural-mix held-out A (1.30) and on every validated-class workload (1.00), and NOT MET on the semi-join-enriched confirmation set B (2.52).** The shortfall is entirely the two Limited metric classes (27 % of B, by design). I did not loosen a gate to hide it. **Unsafe selections: 0 from automatic PostGIS selection on either set**; 2 on A (0 on B) come from the R5d/DuckDB path (huge unfiltered metric semi-joins where DuckDB timed out at 60 s while PostGIS needed 12–18 s) — pre-existing behaviour, not caused by the model, detailed below.
* Correctness: **0 mismatches in 720 cases** (240 + 220 + 260), 17/17 cross-engine plans identical, Demo B 27,624 ids equal to the independent oracle (SHA-256), wide case 189,772 identical on both engines.

## Repository State

| Item | Value |
|---|---|
| Branch / head | `main`, normal commits, no history rewrite |
| Phase commits | `a5dd14b` groundwork + gates, `89bfdc2` model v1 (frozen before held-out), `f2436e5` model v1.1 + A-derived validation (frozen before B), `6d771ac` B evidence + final validation + UI + env sanitization + tests, `801d776` ADR/docs/regression, report commit |
| CI | all jobs green on `801d776`; Pages deployed |
| New crates / modules | `geo-executor::cost_model` (classes, formula evaluation, model/validation file formats), `geo-planner::features`, `geo-engine-postgis::cost`, `geo-engine-duckdb::cost` |
| Tests | Rust workspace 265 passed (was 254); web unit 144 passed (was 137) |

## Benchmark Environment

Recorded by `benchmarks/postgis_env_snapshot.py` (`benchmarks/results/postgis-cost/env-*.json`): Intel i7-14700 (28 logical CPUs), 31.8 GB RAM, Windows 11 10.0.26200, PostgreSQL 16.9 + PostGIS 3.6.2 (GEOS 3.14.1, PROJ 8.2.1) on 127.0.0.1:5433, DuckDB 1.5.6 prebuilt, `cargo --release`. Datasets: Overture 2026-09-23.1; PostGIS rows buildings 3,845,734, transport 453,550, places 449,849, landuse 13,277, admin 325; ANALYZE 2026-10-21 (unchanged since the previously verified generation; I did not rebuild or reload anything). Indexes: GiST on geometry and on the covering envelope for every table, expression GiST on `ST_Transform(geometry, 32648)` for buildings and transport. PostgreSQL settings: shared_buffers 512 MB, work_mem 64 MB, parallel workers per gather 2, JIT on (defaults of the portable install).

Terminology (no "cold cache" is claimed anywhere): the first case after a PostgreSQL restart is **process-cold**; the OS file cache is not controlled; repetitions are **warm-repeat**. Each case: one first run, then 3 warm repetitions (1 above 10 s, 0 above 30 s); the figure used is the warm median. Engine order alternates per case. Load: the user's browsers and editors were open (CPU 4–10 % idle load sampled at the start); this is not a lab-isolated run. Timeouts: 60 s, recorded as `timeout`, never as a time (enter the fit as censored observations ≥ 60 s).

## Benchmark Corpus

| Set | Seed | Cases | Role |
|---|---|---|---|
| calibration | 20261103 | 240 | fit model v1; later also part of v1.1 |
| held-out A | 20261107 | 220 | assessed model v1; decided the initial eligibility; then joined the calibration data for v1.1 |
| confirmation B | 20261111 | 260 | assessed the shipped v1.1 coefficients; confirmed or promoted classes |
| regression | hand written | 5 | Demo B, wide semi-join, small indexed wins, admin semi-join |

None of 20261018, 20261019, 20261026 was reused. Generation is deterministic (`benchmarks/make_postgis_cost_cases.py`); windows are log-uniform from 0.0015° to the whole extent around dense / medium / sparse / broad centres (≈ 25 % each). Shapes: features with window / attribute filter / limit, counts, count_by, intersects, metric near (count and ids) with 25–1000 m and with / without a road-class filter, admin semi-join, sort+limit, aggregate statistics, projected column, unordered limit. Operations covered: Source, Filter, BboxFilter, Project, Sort, Limit, Aggregate, Intersects, Transform, SpatialSemiJoin. Result sizes from 1 row to ≈ 877,000 rows; matching rows from tens to millions. Every comparable pair of results is compared cell by cell and geometries coordinate by coordinate; **0 mismatches**. Failures are recorded as such: 6 / 5 / 12 DuckDB timeouts (calibration / A / B; 12 of B's are admin semi-joins), 3 / 4 / 3 PostGIS "unsupported" (an intersects request whose window leaves the clipped relation's coverage). The raw traces (EXPLAIN text, every run) are not committed (`raw/`, git-ignored); compact per-case files and every evaluation are.

Caveats found on the way and handled, not hidden: (1) the first harness timed `dispatch`; DuckDB's health tracker then refused it after three timeouts, so the harness now times each engine's `execute` directly (identical for both, independent of planner state) and the run was repeated; (2) dev builds distort Arrow/driver time, so all timings are `--release`; (3) the PostGIS relation is a clipped copy and lacks nested columns, so the `near` request (which returns every column) is not PostGIS-eligible — the harness uses the same semi-join returning `id, class, geometry`; (4) `SharedPostGis` did not forward `cost_estimate`, hiding the model from the planner on the first held-out run — fixed and pinned by a test, and the stored cases were re-planned (`GF_COST_PLAN_ONLY`) rather than re-measured, as timings do not depend on the planner.

## Workload Classes

Derived from plan-time features only (`workload_class`): `indexed_bbox`, `indexed_spatial`, `aggregate`, `broad_scan` (rows that must be read > 300,000; an unordered LIMIT stops early and is excluded), `semijoin_intersects`, `semijoin_metric_small` (≤ 5,000 probes), `semijoin_metric_large`. Thresholds were read off the calibration set. Python and Rust assign the same class to all 240 calibration cases (test) and the Rust formula reproduces the Python prediction on 42 fixtures (test).

## Feature Design

`WorkloadFeatures` (planner side, engine independent, nothing observed at run time): per source total rows and window-selected rows; result rows (expected and pessimistic), matching rows, result geometry/attribute bytes; bbox selectivity; flags for filter / sort / aggregate / intersects / transform; limit and whether it is certain; for a semi-join: left rows (= expected probes), right rows, metric or not, predicate, distance, source sizes. Engine-side only: index state from the catalog. Evaluated and rejected: a density-aware candidates-per-probe term for the metric semi-join (no gain), a hinge term for the parallel-plan switch of the admin join (no gain).

## PostGIS Cost Model v1 (shipped as v1.1)

`time_ms = Σ coefficient × term` with terms `startup`, `scan` (per 1000 matching rows read), `result` (per 1000 rows returned), `result_geometry` (per MB), `probe`, `right`; per class, NNLS on relative error (weight 1/max(time, 5 ms)), coefficients non-negative, with provenance (commit, seeds, dataset versions, machine class, build profile, method) in `cost_model_v1.json`. Typical values: startup 12–16 ms (4 ms for large metric semi-joins), ≈ 0.3 ms per 1000 matching rows read for windows and aggregates, 7.1 ms per MB of returned geometry, 4 ms per 1000 probes for the admin join and 25 ms per 1000 probes for small metric semi-joins. A `postgis-cost-v1.1` file, not a black box, ≤ 7 classes × ≤ 5 coefficients. DuckDB keeps its measured shared formula except for semi-join shapes, where `duckdb-semijoin-cost-v1.1` replaces it (that formula was ×26 off on the admin join).

## SemiJoin Risk Model

Cost and safety are separate. The cost model prices a semi-join from probes and right-side rows; the **hard policy** (`geo-engine-postgis::cost::risk`): metric semi-join with > 50,000 expected probes → `blocked` (infeasible for automatic selection; an explicit hint overrides and says so); 5,000–50,000 → `elevated`; index state not `available` → `elevated` and never validated. DuckDB carries a *warning* above 200,000 probes (it timed out in 2 of 11 such measured cases). Why a threshold and not a coefficient: above ≈ 5 k probes the winner flips with right-side density and distance; above 50 k PostGIS took 2–35 s in most measured cases against DuckDB's 0.3–20 s, and the wide 189,772-id window took 12–18 s against 1.0 s.

## Calibration Set

240 cases, 231 comparable. Fit quality (median / P90 ratio of predicted to actual, in-sample, v1): indexed_bbox 1.04 / 1.35, indexed_spatial 1.12 / 1.85, aggregate 1.06 / 2.00, broad_scan 1.07 / 1.42 (n = 9), semijoin_intersects 1.20 / 3.10, metric_small 1.55 / 5.68, metric_large 2.19 / 6.05. PostGIS was faster in 81/81 indexed_bbox, 21/22 indexed_spatial, 56/59 aggregate and 13/13 admin semi-join cases (DuckDB timed out in 6 more), 5/9 broad scans, 16/17 small and 12/30 large metric semi-joins.

## Held-Out Set

A (220 cases, model v1, planner decisions replayed from the logged candidates; the replay agrees with the real planner on 260/260 cases of B). Class decisions under the pre-fixed gates (`benchmarks/postgis_cost_gates.json`, committed before any held-out result existed): **validated** indexed_bbox (n = 70), aggregate (50), indexed_spatial (18); **limited** semijoin_intersects (n = 18, regret 1.00 but median prediction ratio 1.78 > 1.6), broad_scan (P90 regret 1.54, prediction P90 4.7), metric_small (n = 10 < 15), metric_large (P90 regret 3.75, prediction 2.04 / 4.96). The v1.1 refit on calibration + A removed the admin-join bias; B then judged v1.1.

Selection bias, stated: B's results for the three A-validated classes are unbiased (fixed before B ran); `semijoin_intersects` was promoted *on B*, so its B numbers are the evidence for promotion, not an independent estimate.

## Prediction Accuracy

PostGIS predicted vs measured (ratio = max(pred/actual, actual/pred)), v1.1 on B, n = 257: **median 1.21, P90 3.41, P95 4.63, max 18.3**. By class (B):

| Class | n | median | P90 | P95 | max |
|---|---|---|---|---|---|
| indexed_bbox | 73 | 1.03 | 1.66 | 2.09 | 3.2 |
| indexed_spatial | 22 | 1.12 | 2.95 | 3.32 | 3.9 |
| aggregate | 47 | 1.13 | 2.32 | 2.59 | 3.8 |
| semijoin_intersects | 25 | 1.16 | 1.85 | 3.02 | 3.8 |
| broad_scan | 19 | 1.22 | 3.66 | 5.7 | 5.7 |
| semijoin_metric_small | 32 | 2.85 | 5.09 | 7.37 | 11.5 |
| semijoin_metric_large | 39 | 2.33 | 6.84 | 8.15 | 18.3 |

## Planner Accuracy

On B, final validation (4 classes), n = 257 decidable cases, the real planner replayed: optimal (regret ≤ 1.05) in **208 / 257 (81 %)**; PostGIS chosen 167 times. Inside the auto-select scope 167/167 decisions are optimal or within 1.86×.

| Policy (B) | P50 | P90 | P95 | max regret | mean penalty | max penalty |
|---|---|---|---|---|---|---|
| **planner, class-scoped R5d (this phase)** | 1.00 | **2.52** | 3.62 | 14.7 | **205 ms** | 26.3 s |
| existing planner (R5d everywhere = always DuckDB when feasible) | 4.86 | 14.7 | 137 | 411 | 3634 ms | 59.9 s |
| always DuckDB | 4.86 | 14.7 | 137 | 411 | 3634 ms | 59.9 s |
| always PostGIS (ignores residency, risk and capability) | 1.00 | 1.67 | 2.55 | 16.9 | 124 ms | 7.3 s |

Honest reading: "always PostGIS" beats the planner at P90 and in mean penalty on this corpus — because PostGIS wins most metric semi-joins that R5d forces to DuckDB, and the corpus is enriched in them (27 %). It is not a policy that can be shipped (3 plans are unsupported, the relation is a clipped copy, and the hard risk policy would refuse its worst cases). The absolute-penalty column is the fair summary: 205 ms mean against 3.6 s.

On A (natural mix), final validation: P50 1.00, **P90 1.295**, P95 1.89, max 8.35, mean penalty 583 ms, max penalty 48.0 s (the unsafe case below); old behaviour P90 12.1, mean penalty 2.1 s.

## Regret

Split by scope on B: validated-class workloads n = 167: P90 **1.00**, max 1.86; outside the scope (R5d / risk policy) n = 90: P90 4.55, max 14.7. By class (B, planner regret P90 / max): indexed_bbox 1.00 / 1.00, aggregate 1.00 / 1.11, indexed_spatial 1.00 / 1.86, semijoin_intersects 1.00 / 1.00, broad_scan 2.41 / 2.82, metric_small 7.97 / 13.5, metric_large 4.02 / 14.7.

## Absolute Penalties

Validated scope: max 323 ms (indexed_spatial), mean ≤ 17 ms per class. Outside: metric_small max 284 ms (high regret, small stakes); broad_scan max 254 ms; **metric_large max 26.3 s** on B and **48.0 s** on A — the only multi-second penalties, both in the unvalidated metric semi-join class. Each of the eight largest penalties on B was inspected: seven are DuckDB chosen in `semijoin_metric_large` where DuckDB's own model underestimates by ×6–58 (630 ms predicted, 36 s measured); one is a 323 ms indexed_spatial case.

## Engine Crossovers

Practical regions, from the measured cases (not hard-coded except the two class thresholds): indexed windows, aggregates and the admin join — PostGIS wins at every size measured (matching rows up to 300,000; 16 ms vs 100–450 ms; admin join 0.05–1 s vs 1–60 s+). Broad aggregates: between 300 k and 1 M matching rows the two are within about ±30 % of each other; above ≈ 1 M rows DuckDB wins almost every time (1.6 M rows: 1423 ms vs 339 ms; 3.3 M rows: 476–588 ms vs 217–298 ms). Metric semi-join: PostGIS wins below ≈ 5 k probes, the winner flips between 5 k and 50 k, DuckDB usually wins above 50 k, with exceptions in both directions. A crossover plot is not committed (no generated images); the compact case files contain every point.

## Index Evidence

EXPLAIN (ANALYZE, BUFFERS) was captured for every PostGIS run (text; node types, indexes, actual rows/loops, buffers, planning/execution time). Across 710 PostGIS runs (calibration + A + B): indexed_bbox used an index or bitmap scan in 219 of 224 cases (5 sequential scans on tiny relations or very broad windows), aggregate 139 of 156 (the other 17 are large windows where PostgreSQL chose a sequential scan), indexed_spatial 62/62, admin semi-join 62/62 (nested loop on `admin_geometry_gix`), metric semi-joins 163/163 (nested loop on `transport_utm32648_gix`, parallel in 141 of them). Regret was measured on all of these, sequential-scan cases included; the model does not assume an index per query — the catalog index state gates *validation*, and an unindexed source cannot be validated (test).

## R5d Decision

**Option B — scoped by validated workload class.** R5d is unchanged for any candidate whose engine model is not `validated` for the plan's class with an `available` index; it no longer applies when it is. Option C (removal) is not supported by the evidence (metric semi-joins, broad scans, unknown indexes, remote or other-machine use). An explicit engine hint still selects any feasible engine.

## Planner Integration

`EngineDescriptor::cost_estimate` (default `None`), `WorkloadFeatures`, `Candidate.cost_model`, `PhysicalPlan.workload`. Order: authorization → residency → capabilities → health → hard risk policy → cost → class-scoped R5d. PostGIS cost code does not touch `geo-plan` or the canonical lowering (arch_check green). Health, optional-unavailable behaviour and the fallback rules (max one fallback, none after cancellation, no same-engine retry) are untouched; all earlier planner tests pass.

## Developer UI

Per candidate, Developer mode shows: predicted time, workload class, validation level (badge: ok / warn), risk (`low | elevated | high (refused)`), index state, the three largest cost terms, model version and the risk reason (`src/ui/cost-model.ts`, unit tested; strings in VI and EN). Explorer never shows it. Rejection text names the risk policy and states that an explicit request overrides it.

## Correctness Regression

Cross-engine: 17/17 plans IDENTICAL on the slice window (cell by cell, geometries by coordinate); 8/8 live PostGIS driver tests; 0 mismatches over the 720 corpus cases. Request/lowering fixtures and the 11 golden lowering fixtures: Rust workspace 265 passed.

## Demo B

27,624 ids on both engines; PostGIS' id-set SHA-256 equals the independent shapely/pyproj oracle (`5181cea8…533b6`, `benchmarks/results/postgis-cost/oracle-demoB.json`). The planner chooses DuckDB (295 ms vs 1141 ms) — correct.

## Large Metric SemiJoin

The wide window: 189,772 ids on both engines (identical); PostGIS 17.8 s, DuckDB 1.0 s; the planner chooses DuckDB (PostGIS refused by the risk policy). Preserved.

## Static Demo Regression

Public static suite — local Chrome 13/13 and Edge 13/13 (1 opt-in skipped each); live Edge 13/13. Live Chrome: **13/13 only after retries, stated plainly** — three full runs each had 1–2 first-view timeouts (a plain request to GitHub Pages took ~11 s at the time; the CDN range request took 0.17 s), and each of the failing tests passed when re-run alone (the two heaviest first-view tests in 41 s together). Live Edge passed 13/13 in 2.8 min on its first run. The slow first view is the 35 MB DuckDB-Wasm download on this network (measured 18–70 s); the only change that reaches the static bundle is the Developer-mode panel line that renders a cost-model verdict, which the browser-only profile never receives (its candidates carry no `cost_model`).

## CI / Test Environment

CI green on `801d776` (repository guard, architecture, web, Rust workspace, DuckDB start-up repro matrix, experiments). **Environment sanitization**: the full-stack gateway is now started with every `GEOFABRIC_POSTGIS_*` variable blanked (`web/e2e/gateway-env.ts`, opt-in `GF_E2E_POSTGIS=1`), on a fresh gateway (`PW_REUSE_GATEWAY=1` to reuse), pinned by `web/test/e2e-env.test.ts`. Verified the hard way: the shell still exported those variables during the final run, and **full-stack Chrome 61 passed and Edge 61 passed** (4 opt-in skipped each) — the three tests that failed last time pass. The live benchmark is opt-in/manual (`GF_COST_CASES`), the model tests are in normal CI.

## Architecture Risks

1. Single machine, loopback database, one dataset family; no hardware-independence claim. 2. No remote latency calibration, no TLS (production/self-host debt). 3. The cost model's cardinality input is Parquet-footer based; the planner still reads footers for PostGIS sources (PostGIS-only source planning is P1). 4. DuckDB's metric semi-join model underestimates large cases by ×10–60; the first DuckDB abort of the whole benchmark process (one occurrence, exit code `0xffffffff` at the 60 s boundary of a 250 k-probe semi-join, not reproduced in isolation or on rerun) shows the engine can take the gateway process down at the timeout limit — it should be investigated before exposing huge semi-joins to untrusted users. 5. Two class thresholds (300,000 rows; 5,000 probes) and the 50,000-probe block are machine-calibrated constants.

## Follow-up (2026-10-26): connector latency bug, remeasurement, model v2, and the "abort" explained

* **The abort was the RAM killer.** `C:\Users\Admin\Scripts\ram-emergency-killer.log` records "killed postgis_cost-... (2.1 GB)" at 2026-10-23 14:08:43 and 2026-10-26 11:20:59, matching both exits (`0xffffffff` is the exit code of a killed process). A 44-case DuckDB stress run (7 timeouts at 60 s, one process) did not crash. The statement below ("not root-caused") was my first conclusion and is wrong; the P1 is closed. The defect fixed along the way (an interrupted statement was run a second time) was real but not the cause.
* **Connector latency bug** (+ up to 15 ms on every PostGIS query) found, fixed, all three sets re-measured (720 cases, 0 mismatches), model v2 refit, validation reset and re-earned on two new independent sets (seeds 20261115 and 20261119, 300 cases each, 0 mismatches). One gate amendment, made after seeing a result, is documented in `postgis_cost_gates.json` and ADR-015 and was confirmed on a set created after it. Final validated classes: `indexed_bbox`, `aggregate`, `semijoin_intersects`. In scope the planner's P90 regret is 1.00 (max absolute penalty 134 / 376 ms); over all cases P90 regret is 6.99 / 3.11, so the primary gate is still not met, and always-PostGIS beats the planner on mean penalty (154 / 260 ms vs 396 / 473 ms).
* **Scale warning** above 250,000 probes implemented (planner decision log + policy list; a warning, not a refusal).
* **Regression after the connector and planner changes:** 8/8 live PostGIS driver tests (timeout/cancel included), 17/17 cross-engine plans identical, Demo B equal to the independent oracle by SHA-256, Rust 267 passed, web 144 passed, CI and Pages green on `15c092d`; full-stack with a freshly built release gateway: Chrome 61 passed, Edge 61 passed (4 opt-in skipped each). One earlier full-stack attempt ran against a stale gateway binary because the rebuild was blocked by a file lock; it was discarded and repeated.
* **Not done:** a second physical machine and a remote PostgreSQL — none is available to me.

### Original follow-up text (2026-10-25, superseded)

Investigated, **not reproduced and not root-caused**. The one abort (exit `0xffffffff`, case hel-0170, ~250k probes, at the 60 s boundary after 170 cases in one process) did not recur in isolation or on rerun, nor in 15 fresh-process runs of the same case with a 3 s timeout (0/15). One real defect was found on that path and fixed: when a statement was interrupted by the timeout or a cancel, the engine **ran the whole statement a second time** (to read DuckDB's error message) while the watchdog kept interrupting it. An interrupted statement is no longer re-run (`geo-engine-duckdb`, the existing timeout/cancel tests pass). Whether that second execution was the trigger is unknown; the P1 stays open until a 60 s-boundary stress run (many cases, one process) is clean, which I have not run because it takes tens of minutes.

## Remaining P0

None known.

## Remaining P1

PostGIS-only source planning; promotion of `semijoin_metric_small` (regret is acceptable when PostGIS is chosen, P90 1.32, but the numeric fit fails the pre-declared gates: needs a better model or a documented, freshly confirmed gate amendment); a physical-design experiment for the metric semi-join (two-stage prefilter, generated projected column) — not touched; a second physical machine; remote PostgreSQL with real latency; TLS; investigate the DuckDB abort at the timeout boundary; a user-visible warning for plans above ≈ 250 k probes (neither engine is reliably safe).

## Recommended Next Phase

Second-device and remote-database calibration of the same corpus (the harness and fit are reusable), then — only with a better feature for the metric semi-join — a fresh-set confirmation of `semijoin_metric_small`.

## Final Decision Matrix

| Workload class | PostGIS model | Auto-select | Evidence |
|---|---|---|---|
| indexed_bbox (selective window, filter, sort, limit) | validated | **yes** | B n=73, regret 1.00, prediction 1.03 / 1.66; A n=70 |
| indexed_spatial (intersects, projected column) | validated | **yes** | B n=22, regret P90 1.00 (max 1.86, 323 ms); A n=18 |
| aggregate (≤ 300,000 rows) | validated | **yes** | B n=47, regret 1.00, prediction 1.13 / 2.32; A n=50 |
| semijoin_intersects (admin) | validated | **yes** | B n=25 (12 DuckDB timeouts), regret 1.00, prediction 1.16 / 1.85 — promoted on B; A n=18 regret 1.00 |
| broad_scan (> 300,000 rows read) | limited | no | B P90 regret 2.41; prediction P90 3.66 > 3.0 |
| semijoin_metric_small (≤ 5,000 probes) | limited | no | B regret P90 1.32 if chosen, prediction 2.85 / 5.09 fail the numeric gates |
| semijoin_metric_large (> 5,000 probes) | limited, blocked > 50,000 | no | B P90 regret 3.6, prediction 2.33 / 6.84; winner flips with density/distance |
| unknown or unavailable index state | unvalidated | no | by rule (catalog `spatially_indexed` / `metric_indexes`) |
| remote PostGIS, other hardware | not evaluated | no | out of scope |
