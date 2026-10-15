# REPORT-GEOFABRIC-PLANNER-HARDENING

Phase: planner hardening (probe · estimator v2 · held-out evaluation · network model · health/fallback · confidence · UI). Date: 2026-10-14/15. Branch `main`, commit @@COMMIT@@. **Stopped for review; no PostGIS work was started.**

## 1. Executive Summary

| Question | Answer |
|---|---|
| Does the planner now know which engine *can* run? | **Yes, by measurement of the runtime, not by description.** The browser runs a 16-check probe (real compiler + real streaming path, two fixture files, one tiny plan per operator, range requests, covering filter, memory class) in 2.0 s cold / 39 ms warm; failed required checks, unverified operations, unreadable file classes, failed range requests, unhealthy state or a known incompatibility pre-reject the browser with the reason in the plan. |
| Which *should*? | Cost model with an explicit network term; browser access strategy chosen per source (whole-file download ≤ 32 MB, range requests above). Held-out: **accuracy 98 %, P90 regret 1.00 (max 1.08)** over 100 cases — **but always-server scores 96 %**: the browser is fastest in 4 of 100 held-out cases (all on LOCAL). The headline number says little about the browser branch. |
| How *confident*? | 0–1 score from 7 reasoned factors × a margin factor; explicit policy R5b (never choose an unprobed browser on cost alone; never below 0.45; never on a < 20 % cost advantage when it is less trusted). Accuracy by confidence bin: 94 % (0.65–0.80, n=35), 100 % (≥ 0.80, n=64). Bins are small. |
| What happens when it is wrong? | Taxonomy of 8 failure classes (Rust + TypeScript classifiers checked against one fixture), health state per engine, known incompatibilities remembered, **at most one fallback** (browser → server, same logical plan, reason shown), no retry, no fallback on cancellation. Held-out: 0 browser crashes, 0 digest mismatches (29 deterministic cases) and 0 row-count mismatches. |
| Targets | **Estimator: median 1.12× (< 2 ✔), P90 11.5× (< 5 ✘ missed), max 29×.** Time model: median 1.08×/1.09× (server/browser), P90 1.3×/2.7×. Planner accuracy ≥ 85 % ✔ (98 %), P90 regret ≤ 1.25 ✔ (1.00). |

**Two findings changed what the project believed** (both ADR-009, both found by the probe/measurement work, not by review):

1. DuckDB-Wasm was **downloading every file whole** (one `GET` without `Range`, status 200, 98 MB for a 100 MB file, on every query). All browser measurements of the previous phase — the cost-model fit, "no struct-statistics pruning", the 245 MB "crash threshold" — measured whole-file downloads. Fixed with the engine's filesystem configuration; range requests turn out to be strictly sequential (~9.6 requests per touched row group, ~14.5 ms each on loopback), so the planner now picks `full` or `range` per source.
2. "DuckDB-Wasm cannot read Overture's typed Parquet GEOMETRY" was **wrong**: the TypeScript lowering streamed a `GEOMETRY('OGC:CRS84')` column, which DuckDB-Wasm cannot do (`ActiveTransaction called without active transaction`; `conn.query` works, `conn.send` fails; `ST_AsWKB` at the scan fixes it). The Overture subset now runs in the browser with digests equal to the server. The planner had been routing around a bug in our own code.

**Recommendation: KEEP HARDENING PLANNER** (section 17).

## 2. Repository State

- `main` @@COMMIT@@; working tree clean. New: `crates/geo-planner/src/{failure,probe,network,estimator,confidence,state}.rs`; gateway endpoints `/api/{probe,probe/invalidate,report,state,state/reset,telemetry,stats,network}` and flags `--network --simulate-network --telemetry --no-learn`; web `engine/{probe,probe-model,failure}.ts`; Execution Plan panel; held-out harness (`web/e2e/heldout.spec.ts`, `web/playwright.heldout.config.ts`, `benchmarks/{make_heldout_cases,make_cal2_cases,make_netcal_cases,build_calibration,run_heldout,analyze_heldout,make_report_tables,nnls_fit}.py`); fixtures `web/public/probe/*.parquet` (8 KB each); ADR-009, ADR-010, RFC-004 (proposal), ADR-008 correction, RFC-003 v1.1 section; `docs/claude/planner-hardening.md`.
- IR 0.2 / PlanGraph: unchanged (no serialized-format change). `EngineDescriptor`/`ExecutionEngine` separation kept.
- Data: existing synthetic 10/100 MB/1 GB files and the 1.45 GB Overture subset; one new generator size, 30 MB (4 files, 30 MB each) — a size nothing was fitted on. No 10 GB, no PostGIS, gateway still localhost-only without auth (the new `/api/state/reset` is a dev/test aid under the same trust model).
- Codex: not used this phase (the work was integration-heavy and the small tasks would have cost more review than writing).

## 3. Runtime Capability Probe

Three layers (ADR-009): static capabilities → runtime probe → observed health. Checks (all in a real Chrome, headless, DuckDB-Wasm 1.33.1-dev57, DuckDB core as reported by `version()`): `worker`, `init`, `arrow`, `extension_load` (information), `parquet`, `http_range`, `geometry_wkb`, `covering`, `read_class:plain_wkb`, `read_class:duckdb_typed`, `op:{source,filter,bbox_filter,aggregate,project,limit,sort}`. Required: worker, init, arrow, parquet, geometry_wkb, covering. Also reported: engine/runtime version, memory class from `navigator.deviceMemory`, JS heap limit, supported operations, duration. Cost: 2.0 s on a cold page (it includes DuckDB-Wasm instantiation, which the first browser query would pay anyway), 39 ms warm.
Cache and invalidation: per page session (reload ⇒ new session replaces the old report); client re-probes after 25 min, planner calls a report > 30 min old *stale*; engine version change clears health and incompatibilities; a crash/OOM removes the probe and the client probes the fresh instance. Unprobed ⇒ never chosen on cost alone (R5b).
Evidence: probe passes on this machine with all 16 checks; unit/integration tests drive each rejection reason (12 hardening tests on real footers); real-browser tests (7) cover probe→gateway→planner, pre-rejection, UI, bounded fallback.
Limit: one machine, one browser; the probe cannot prove that a 1 GB file fits in memory.

## 4. Engine Health

`Healthy / Degraded / Unhealthy / Unknown` per engine (last 12 outcomes within 15 min; crash/OOM ⇒ Unhealthy for a 30 s cool-down, then Degraded until a success; 3 consecutive failures ⇒ Unhealthy; ≥ 25 % failures ⇒ Degraded). Deterministic failures and cancellations do not touch health; deterministic failures become *known incompatibilities* (engine × source), never retried. Unhealthy is a hard filter unless every engine would be filtered. Server runs feed the same tracker (the gateway records timeouts and engine errors; user errors and limits are not statements about the engine). Tested at unit level (6), planner level (health/incompat tests) and in Chrome (crash report ⇒ unhealthy, probe invalidated, browser rejected in the next plan).

## 5. Estimator v2

Statistical and explainable (ADR-010 §5): footer statistics (rows, row groups, bbox extents, covering quality declared/inferred/none, spatial coherence, density skew, null fraction) + a calibration learned from telemetry (shrunk median of past `actual/raw`, per source and selectivity bucket, censored LIMIT runs excluded; aggregate group counts). Output: rows, interval, quality with reasons. It never sees the outcome of the query it estimates; held-out runs use `--no-learn`. Calibration data: 48 observations from the crossover grid (4 datasets × 4 windows × 3 queries, LOCAL). *Not done:* column-statistics histograms (the footers of these files carry none).

## 6. Estimator Accuracy

### Estimator accuracy (result rows, held-out, n=100)

| estimator | median × | P90 × | max × | over-estimates | under-estimates | actual inside [low, high] |
|---|---|---|---|---|---|---|
| v1-equivalent (no calibration) | 1.09 | 15.0 | 267.33 | 41% | 11% | 85% |
| v2 (calibrated on the calibration set) | 1.12 | 11.47 | 29.47 | 27% | 26% | 71% |

By plan shape (v2): count: median 1.0×, P90 1.0×, max 1.0× (n=9); count_by: median 1.12×, P90 9.0×, max 9.0× (n=35); features: median 1.0×, P90 11.89×, max 29.47× (n=56)

By self-reported quality (v2): fair: median 9.0×, P90 11.89×, max 23.33× (n=33); good: median 1.0×, P90 2.04×, max 29.47× (n=38); poor: median 1.12×, P90 9.0×, max 15.0× (n=29)

#### Worst 12 cases and a sample of the rest

| case | dataset | query | window | limit | actual rows | estimated | interval | quality / basis | error × |
|---|---|---|---|---|---|---|---|---|---|
| ho037 | clustered_30mb_shuffled | features | 0.02 | 500 | 16 | 500 | 250–500 | good / none | 29.5 |
| ho045 | clustered_100mb_shuffled | features | 0.0005 | 2000 | 2 | 69 | 17–274 | fair / global | 23.3 |
| ho001 | clustered_30mb_shuffled | features | 0.0005 | 500 | 0 | 14 | 1–139 | poor / none | 15.0 |
| ho023 | clustered_30mb_shuffled | features | 0.0005 | 2000 | 0 | 14 | 1–139 | poor / none | 15.0 |
| ho043 | uniform_100mb_spatial | features | 0.0005 | 2000 | 657 | 53 | 13–210 | fair / global | 12.2 |
| ho028 | uniform_30mb_shuffled | features | 0.0005 | 500 | 213 | 17 | 4–67 | fair / global | 11.9 |
| ho039 | uniform_30mb_shuffled | features | 0.0005 | 20000 | 213 | 17 | 4–67 | fair / global | 11.9 |
| ho095 | uniform_30mb_shuffled | features | 0.0005 | 2000 | 213 | 17 | 4–67 | fair / global | 11.9 |
| ho067 | uniform_30mb_shuffled | features | 0.0005 | 500 | 213 | 17 | 4–67 | fair / global | 11.9 |
| ho049 | uniform_30mb_spatial | features | 0.0005 | 20000 | 194 | 16 | 3–61 | fair / global | 11.5 |
| ho094 | uniform_30mb_spatial | features | 0.0005 | 2000 | 194 | 16 | 3–61 | fair / global | 11.5 |
| ho077 | uniform_30mb_spatial | features | 0.0005 | 500 | 194 | 16 | 3–61 | fair / global | 11.5 |

<details><summary>All 100 cases</summary>

| case | dataset | query | window | limit | actual | est | error × |
|---|---|---|---|---|---|---|---|
| ho000 | clustered_30mb_spatial | count_by | 0.02 |  | 8 | 8 | 1.0 |
| ho001 | clustered_30mb_shuffled | features | 0.0005 | 500 | 0 | 14 | 15.0 |
| ho002 | uniform_30mb_spatial | features | 0.02 | 2000 | 982 | 2000 | 2.0 |
| ho003 | uniform_30mb_shuffled | count_by | 0.5 |  | 8 | 8 | 1.0 |
| ho004 | uniform_10mb_spatial | features | 0.5 | 500 | 500 | 500 | 1.0 |
| ho005 | uniform_10mb_shuffled | features | 0.02 | 500 | 500 | 500 | 1.0 |
| ho006 | clustered_10mb_shuffled | features | 0.1 | 2000 | 2000 | 2000 | 1.0 |
| ho007 | uniform_100mb_spatial | count_by | 0.02 |  | 50 | 8 | 5.7 |
| ho008 | uniform_100mb_shuffled | features | 0.02 | 500 | 500 | 500 | 1.0 |
| ho009 | clustered_100mb_shuffled | count_by | 0.0005 |  | 7 | 8 | 1.1 |
| ho010 | uniform_1gb_spatial | count_by | 0.5 |  | 7 | 8 | 1.1 |
| ho011 | clustered_30mb_spatial | count_by | 0.0005 |  | 0 | 8 | 9.0 |
| ho012 | clustered_30mb_shuffled | count_by | 0.5 |  | 8 | 8 | 1.0 |
| ho013 | uniform_30mb_spatial | features | 0.5 | 20000 | 20000 | 20000 | 1.0 |
| ho014 | uniform_30mb_shuffled | features | 0.02 | 2000 | 919 | 2000 | 2.2 |
| ho015 | uniform_10mb_spatial | features | 0.02 | 2000 | 2000 | 2000 | 1.0 |
| ho016 | uniform_10mb_shuffled | features | 0.0005 | 2000 | 13 | 6 | 2.0 |
| ho017 | clustered_10mb_shuffled | count_by | 0.1 |  | 8 | 8 | 1.0 |
| ho018 | uniform_100mb_spatial | count_by | 0.5 |  | 7 | 8 | 1.1 |
| ho019 | uniform_100mb_shuffled | count_by | 0.1 |  | 8 | 8 | 1.0 |
| ho020 | clustered_100mb_shuffled | features | 0.1 | 20000 | 20000 | 20000 | 1.0 |
| ho021 | uniform_1gb_spatial | features | 0.5 | 2000 | 2000 | 2000 | 1.0 |
| ho022 | clustered_30mb_spatial | features | 0.02 | 20000 | 4093 | 393 | 10.4 |
| ho023 | clustered_30mb_shuffled | features | 0.0005 | 2000 | 0 | 14 | 15.0 |
| ho024 | uniform_30mb_spatial | features | 0.0005 | 2000 | 25 | 16 | 1.5 |
| ho025 | uniform_30mb_shuffled | count_by | 0.0005 |  | 8 | 8 | 1.0 |
| ho026 | uniform_10mb_spatial | count | 0.1 |  | 1 | 1 | 1.0 |
| ho027 | uniform_10mb_shuffled | features | 0.0005 | 2000 | 74 | 6 | 10.7 |
| ho028 | uniform_30mb_shuffled | features | 0.0005 | 500 | 213 | 17 | 11.9 |
| ho029 | uniform_10mb_spatial | count_by | 0.02 |  | 50 | 8 | 5.7 |
| ho030 | uniform_10mb_shuffled | features | 0.0005 | 2000 | 74 | 6 | 10.7 |
| ho031 | clustered_10mb_shuffled | features | 0.1 | 2000 | 2000 | 2000 | 1.0 |
| ho032 | uniform_100mb_spatial | count_by | 0.1 |  | 50 | 8 | 5.7 |
| ho033 | uniform_100mb_shuffled | count_by | 0.1 |  | 7 | 8 | 1.1 |
| ho034 | clustered_100mb_shuffled | count | 0.1 |  | 1 | 1 | 1.0 |
| ho035 | clustered_30mb_spatial | features | 0.0005 | 20000 | 0 | 8 | 9.0 |
| ho036 | clustered_30mb_spatial | features | 0.02 | 2000 | 519 | 393 | 1.3 |
| ho037 | clustered_30mb_shuffled | features | 0.02 | 500 | 16 | 500 | 29.5 |
| ho038 | uniform_30mb_spatial | count_by | 0.5 |  | 50 | 8 | 5.7 |
| ho039 | uniform_30mb_shuffled | features | 0.0005 | 20000 | 213 | 17 | 11.9 |
| ho040 | uniform_10mb_spatial | count | 0.5 |  | 1 | 1 | 1.0 |
| ho041 | uniform_10mb_shuffled | features | 0.5 | 500 | 500 | 500 | 1.0 |
| ho042 | clustered_10mb_shuffled | features | 0.1 | 2000 | 2000 | 2000 | 1.0 |
| ho043 | uniform_100mb_spatial | features | 0.0005 | 2000 | 657 | 53 | 12.2 |
| ho044 | uniform_100mb_shuffled | count_by | 0.0005 |  | 50 | 8 | 5.7 |
| ho045 | clustered_100mb_shuffled | features | 0.0005 | 2000 | 2 | 69 | 23.3 |
| ho046 | clustered_30mb_spatial | count_by | 0.0005 |  | 0 | 8 | 9.0 |
| ho047 | clustered_30mb_spatial | features | 0.02 | 2000 | 2000 | 393 | 5.1 |
| ho048 | clustered_30mb_shuffled | count_by | 0.0005 |  | 0 | 8 | 9.0 |
| ho049 | uniform_30mb_spatial | features | 0.0005 | 20000 | 194 | 16 | 11.5 |
| ho050 | uniform_30mb_shuffled | features | 0.02 | 20000 | 7801 | 7801 | 1.0 |
| ho051 | uniform_10mb_spatial | features | 0.0005 | 500 | 59 | 6 | 8.6 |
| ho052 | uniform_10mb_shuffled | count_by | 0.5 |  | 50 | 8 | 5.7 |
| ho053 | clustered_10mb_shuffled | features | 0.5 | 500 | 500 | 500 | 1.0 |
| ho054 | uniform_100mb_spatial | features | 0.1 | 20000 | 20000 | 20000 | 1.0 |
| ho055 | uniform_100mb_shuffled | count_by | 0.5 |  | 8 | 8 | 1.0 |
| ho056 | clustered_100mb_shuffled | count_by | 0.5 |  | 7 | 8 | 1.1 |
| ho057 | clustered_30mb_spatial | count | 0.1 |  | 1 | 1 | 1.0 |
| ho058 | clustered_30mb_spatial | count_by | 0.0005 |  | 0 | 8 | 9.0 |
| ho059 | clustered_30mb_shuffled | count_by | 0.1 |  | 7 | 8 | 1.1 |
| ho060 | uniform_30mb_spatial | count | 0.02 |  | 1 | 1 | 1.0 |
| ho061 | uniform_30mb_shuffled | features | 0.02 | 2000 | 919 | 2000 | 2.2 |
| ho062 | clustered_100mb_shuffled | features | 0.02 | 2000 | 2000 | 2000 | 1.0 |
| ho063 | clustered_30mb_spatial | count | 0.0005 |  | 1 | 1 | 1.0 |
| ho064 | clustered_30mb_spatial | count_by | 0.02 |  | 7 | 8 | 1.1 |
| ho065 | clustered_30mb_shuffled | count | 0.02 |  | 1 | 1 | 1.0 |
| ho066 | uniform_30mb_spatial | features | 0.02 | 500 | 500 | 500 | 1.0 |
| ho067 | uniform_30mb_shuffled | features | 0.0005 | 500 | 213 | 17 | 11.9 |
| ho068 | uniform_10mb_spatial | features | 0.5 | 2000 | 2000 | 2000 | 1.0 |
| ho069 | uniform_10mb_shuffled | features | 0.5 | 20000 | 20000 | 20000 | 1.0 |
| ho070 | clustered_10mb_shuffled | count_by | 0.0005 |  | 0 | 8 | 9.0 |
| ho071 | uniform_100mb_spatial | features | 0.5 | 2000 | 2000 | 2000 | 1.0 |
| ho072 | uniform_100mb_shuffled | count_by | 0.02 |  | 50 | 8 | 5.7 |
| ho073 | clustered_100mb_shuffled | count_by | 0.1 |  | 7 | 8 | 1.1 |
| ho074 | uniform_1gb_spatial | count_by | 0.0005 |  | 8 | 8 | 1.0 |
| ho075 | clustered_30mb_spatial | count_by | 0.5 |  | 8 | 8 | 1.0 |
| ho076 | clustered_30mb_shuffled | count_by | 0.02 |  | 7 | 8 | 1.1 |
| ho077 | uniform_30mb_spatial | features | 0.0005 | 500 | 194 | 16 | 11.5 |
| ho078 | uniform_30mb_shuffled | features | 0.02 | 20000 | 7801 | 7801 | 1.0 |
| ho079 | uniform_10mb_spatial | features | 0.0005 | 500 | 59 | 6 | 8.6 |
| ho080 | uniform_10mb_shuffled | features | 0.02 | 500 | 500 | 500 | 1.0 |
| ho081 | clustered_10mb_shuffled | count_by | 0.02 |  | 7 | 8 | 1.1 |
| ho082 | clustered_30mb_shuffled | count | 0.02 |  | 1 | 1 | 1.0 |
| ho083 | uniform_30mb_spatial | count_by | 0.02 |  | 7 | 8 | 1.1 |
| ho084 | uniform_30mb_shuffled | features | 0.1 | 2000 | 2000 | 2000 | 1.0 |
| ho085 | uniform_10mb_spatial | count_by | 0.02 |  | 50 | 8 | 5.7 |
| ho086 | uniform_10mb_shuffled | features | 0.5 | 20000 | 20000 | 20000 | 1.0 |
| ho087 | clustered_10mb_shuffled | count_by | 0.0005 |  | 0 | 8 | 9.0 |
| ho088 | uniform_100mb_spatial | count | 0.02 |  | 1 | 1 | 1.0 |
| ho089 | uniform_100mb_shuffled | features | 0.5 | 2000 | 2000 | 2000 | 1.0 |
| ho090 | clustered_100mb_shuffled | count_by | 0.0005 |  | 7 | 8 | 1.1 |
| ho091 | clustered_30mb_spatial | features | 0.02 | 500 | 500 | 393 | 1.3 |
| ho092 | clustered_30mb_spatial | features | 0.1 | 20000 | 20000 | 20000 | 1.0 |
| ho093 | clustered_30mb_shuffled | features | 0.5 | 2000 | 2000 | 2000 | 1.0 |
| ho094 | uniform_30mb_spatial | features | 0.0005 | 2000 | 194 | 16 | 11.5 |
| ho095 | uniform_30mb_shuffled | features | 0.0005 | 2000 | 213 | 17 | 11.9 |
| ho096 | uniform_10mb_spatial | features | 0.5 | 500 | 500 | 500 | 1.0 |
| ho097 | uniform_10mb_shuffled | features | 0.5 | 2000 | 2000 | 2000 | 1.0 |
| ho098 | clustered_10mb_shuffled | features | 0.0005 | 20000 | 0 | 8 | 9.0 |
| ho099 | uniform_100mb_spatial | features | 0.5 | 500 | 500 | 500 | 1.0 |

</details>

### Time model accuracy (planner score vs measured end-to-end time)

| engine | n | median × | P90 × | max × | over-estimates | under-estimates |
|---|---|---|---|---|---|---|
| duckdb | 100 | 1.08 | 1.3 | 1.87 | 15% | 85% |
| duckdb-wasm | 96 | 1.09 | 2.69 | 4.13 | 51% | 49% |

By network profile (median × / P90 ×): FAST_LAN: server 1.04/1.33, browser 1.91/3.56; GOOD_BROADBAND: server 1.08/1.35, browser 1.03/2.69; HIGH_LATENCY: server 1.06/1.24, browser 1.01/1.36; LOCAL: server 1.13/1.33, browser 1.38/3.8; MOBILE: server 1.06/1.32, browser 1.02/2.15

Reading: the median is fine, the tail is not. Largest errors come from the *calibration itself generalising badly*: corrections learned on **clustered** files push the estimate the wrong way on **uniform** files (estimated 17 rows, actual 213 at window 0.0005), and a LIMIT-bound estimate is declared "good" on a **shuffled** clustered file where the uniform-density assumption is simply wrong (estimated 500, actual 16). Calibration roughly halves the tail against the uncalibrated estimator (P90 15 → 11.5, max 267 → 29) but misses the < 5× target. I did not tune on these cases (the rule); candidate fixes are in section 17. The self-reported quality is informative for the median case ("good": median 1.0×) but not for the worst case (a "good" max of 29×).

## 7. Network Model

Profiles LOCAL / FAST_LAN (5 ms, 1 Gbps) / GOOD_BROADBAND (20 ms, 200 Mbps) / MOBILE (70 ms, 30 Mbps) / HIGH_LATENCY (150 ms, 20 Mbps); each states whether it is a preset (assumption), configured or measured (only LOCAL is measured, on this host). `server = compute + RTT + upload + result/BW`; `browser = compute + RTT (plan) + sequential requests × RTT + fetched bytes/BW` (+ 1.4 s when the instance is cold). The fitted compute terms contain no network.
Browser range-mode compute terms were refitted on the calibration grid: `19 ms + 14.5 ms/request + 14.1 ms/MB + 104 µs/row` (27 runs, median error 1.04× when the true request/byte counts are given); requests and bytes are predicted from touched row groups (aggregate: 9.6 requests/row group, 47 % of the kept bytes; features: 12 requests per row group the LIMIT needs).

## 8. Held-out Benchmark

Protocol. Calibration set = what the cost model/estimator were fitted on or learn from: the crossover grid (clustered 10 MB/100 MB/1 GB + the Overture subset; windows 0.002–0.25; LOCAL) and its re-run on three networks (`cal2`, 142 cases). Held-out set (`heldout-cases.json`, seed 20261015, generated by `make_heldout_cases.py`) differs on every axis: file size (30 MB new; 10/100 MB/1 GB other layouts), distribution (uniform vs clustered), layout (sorted vs spatially shuffled), selectivity (0.0005, 0.02, 0.1, 0.5 — none in the calibration set), cardinality (GROUP BY category vs height), operation (ungrouped COUNT, attribute filter + features, attribute filter + count_by), browser state (74 warm / 26 cold, cold = instance recycled), network (28 LOCAL, 14 FAST_LAN, 20 GOOD_BROADBAND, 20 MOBILE, 18 HIGH_LATENCY). The gateway ran with the calibration set loaded and `--no-learn`; every case ran 3× per engine (first discarded), end to end through a real Chrome tab, so server and browser times are comparable; cross-engine digests/row counts were checked.
Honesty about the history of this set. An *exploratory* first set (seed 20261014) was run before the two bugs of the summary were understood; its results are not used (its cases and partial results are kept in `benchmarks/results/exploratory-ho1/` for transparency). That exploration is what exposed (a) a bandwidth-accounting bug in my network simulation (streamed file bodies were charged 0 bytes), and (b) the whole-file downloads. After fixing both, the model was refitted **on the calibration set only**, and the reported set was generated with a new seed and evaluated once with frozen code. I looked at the exploratory set before refitting, so it is not a pristine blind set for the *architecture* changes (range/whole-file strategy), only for the *constants*.

## 9. Planner Accuracy

### Planner accuracy and regret (held-out, 100 cases)

accuracy **98.0%** · regret median 1.0 · **P90 1.0** · max 1.08 · browser fastest in 4 cases, chosen in 4

Baselines: always-server accuracy 96.0%, P90 regret 1.0; always-browser (where measurable) median regret 9.518×, P90 49.029×.

| network | cases | accuracy | browser fastest |
|---|---|---|---|
| FAST_LAN | 14 | 100% | 0 |
| GOOD_BROADBAND | 20 | 100% | 0 |
| HIGH_LATENCY | 18 | 100% | 0 |
| LOCAL | 28 | 93% | 4 |
| MOBILE | 20 | 100% | 0 |

| browser state | cases | accuracy |
|---|---|---|
| warm | 74 | 97% |
| cold | 26 | 100% |

| confidence | cases | accuracy | mean confidence |
|---|---|---|---|
| 0.00-0.50 | 0 | 0% | None |
| 0.50-0.65 | 1 | 100% | 0.62 |
| 0.65-0.80 | 35 | 94% | 0.73 |
| 0.80-1.00 | 64 | 100% | 0.89 |

#### Cases where the browser was measured faster, or the planner was wrong, or the browser was chosen

| case | network | dataset | query | window | state | selected | fastest | server ms | browser ms | correct | regret | confidence |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| ho004 | LOCAL | uniform_10mb_spatial | features | 0.5 | warm | duckdb | duckdb-wasm | 111 | 107 | **no** | 1.04 | 0.65 |
| ho026 | LOCAL | uniform_10mb_spatial | count | 0.1 | warm | duckdb-wasm | duckdb-wasm | 99 | 75 | yes | 1.00 | 0.81 |
| ho016 | LOCAL | uniform_10mb_shuffled | features | 0.0005 | warm | duckdb-wasm | duckdb-wasm | 110 | 92 | yes | 1.00 | 0.71 |
| ho027 | LOCAL | uniform_10mb_shuffled | features | 0.0005 | warm | duckdb-wasm | duckdb | 92 | 100 | **no** | 1.08 | 0.71 |
| ho017 | LOCAL | clustered_10mb_shuffled | count_by | 0.1 | warm | duckdb-wasm | duckdb-wasm | 98 | 75 | yes | 1.00 | 0.62 |

<details><summary>All 100 cases</summary>

| case | network | dataset | query | window | state | selected | fastest | server ms | browser ms | browser status | correct | regret | confidence |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| ho000 | LOCAL | clustered_30mb_spatial | count_by | 0.02 | warm | duckdb | duckdb | 97 | 108 | measured | yes | 1.00 | 0.74 |
| ho001 | LOCAL | clustered_30mb_shuffled | features | 0.0005 | cold | duckdb | duckdb | 92 | 667 | measured | yes | 1.00 | 0.76 |
| ho002 | LOCAL | uniform_30mb_spatial | features | 0.02 | warm | duckdb | duckdb | 98 | 203 | measured | yes | 1.00 | 0.95 |
| ho003 | LOCAL | uniform_30mb_shuffled | count_by | 0.5 | warm | duckdb | duckdb | 99 | 137 | measured | yes | 1.00 | 0.67 |
| ho004 | LOCAL | uniform_10mb_spatial | features | 0.5 | warm | duckdb | duckdb-wasm | 111 | 107 | measured | NO | 1.04 | 0.65 |
| ho005 | LOCAL | uniform_10mb_shuffled | features | 0.02 | warm | duckdb | duckdb | 104 | 129 | measured | yes | 1.00 | 0.66 |
| ho006 | LOCAL | clustered_10mb_shuffled | features | 0.1 | warm | duckdb | duckdb | 122 | 271 | measured | yes | 1.00 | 0.95 |
| ho007 | LOCAL | uniform_100mb_spatial | count_by | 0.02 | warm | duckdb | duckdb | 97 | 672 | measured | yes | 1.00 | 0.76 |
| ho008 | LOCAL | uniform_100mb_shuffled | features | 0.02 | cold | duckdb | duckdb | 111 | 779 | measured | yes | 1.00 | 0.95 |
| ho009 | LOCAL | clustered_100mb_shuffled | count_by | 0.0005 | warm | duckdb | duckdb | 110 | 2405 | measured | yes | 1.00 | 0.87 |
| ho010 | LOCAL | uniform_1gb_spatial | count_by | 0.5 | cold | duckdb | duckdb | 309 | None | skipped (tab limit) | yes | 1.00 | 0.76 |
| ho011 | LOCAL | clustered_30mb_spatial | count_by | 0.0005 | warm | duckdb | duckdb | 95 | 133 | measured | yes | 1.00 | 0.74 |
| ho012 | LOCAL | clustered_30mb_shuffled | count_by | 0.5 | warm | duckdb | duckdb | 107 | 150 | measured | yes | 1.00 | 0.67 |
| ho013 | LOCAL | uniform_30mb_spatial | features | 0.5 | warm | duckdb | duckdb | 385 | 1895 | measured | yes | 1.00 | 0.95 |
| ho014 | LOCAL | uniform_30mb_shuffled | features | 0.02 | cold | duckdb | duckdb | 132 | 790 | measured | yes | 1.00 | 0.95 |
| ho015 | LOCAL | uniform_10mb_spatial | features | 0.02 | warm | duckdb | duckdb | 124 | 241 | measured | yes | 1.00 | 0.76 |
| ho016 | LOCAL | uniform_10mb_shuffled | features | 0.0005 | warm | duckdb-wasm | duckdb-wasm | 110 | 92 | measured | yes | 1.00 | 0.71 |
| ho017 | LOCAL | clustered_10mb_shuffled | count_by | 0.1 | warm | duckdb-wasm | duckdb-wasm | 98 | 75 | measured | yes | 1.00 | 0.62 |
| ho018 | LOCAL | uniform_100mb_spatial | count_by | 0.5 | warm | duckdb | duckdb | 110 | 2143 | measured | yes | 1.00 | 0.76 |
| ho019 | LOCAL | uniform_100mb_shuffled | count_by | 0.1 | cold | duckdb | duckdb | 108 | 2712 | measured | yes | 1.00 | 0.76 |
| ho020 | LOCAL | clustered_100mb_shuffled | features | 0.1 | warm | duckdb | duckdb | 528 | 2857 | measured | yes | 1.00 | 0.95 |
| ho021 | LOCAL | uniform_1gb_spatial | features | 0.5 | warm | duckdb | duckdb | 143 | 584 | measured | yes | 1.00 | 0.95 |
| ho022 | LOCAL | clustered_30mb_spatial | features | 0.02 | warm | duckdb | duckdb | 161 | 475 | measured | yes | 1.00 | 0.82 |
| ho023 | LOCAL | clustered_30mb_shuffled | features | 0.0005 | warm | duckdb | duckdb | 96 | 120 | measured | yes | 1.00 | 0.66 |
| ho024 | LOCAL | uniform_30mb_spatial | features | 0.0005 | warm | duckdb | duckdb | 99 | 109 | measured | yes | 1.00 | 0.74 |
| ho025 | LOCAL | uniform_30mb_shuffled | count_by | 0.0005 | cold | duckdb | duckdb | 96 | 694 | measured | yes | 1.00 | 0.87 |
| ho026 | LOCAL | uniform_10mb_spatial | count | 0.1 | warm | duckdb-wasm | duckdb-wasm | 99 | 75 | measured | yes | 1.00 | 0.81 |
| ho027 | LOCAL | uniform_10mb_shuffled | features | 0.0005 | warm | duckdb-wasm | duckdb | 92 | 100 | measured | NO | 1.08 | 0.71 |
| ho028 | FAST_LAN | uniform_30mb_shuffled | features | 0.0005 | cold | duckdb | duckdb | 127 | 992 | measured | yes | 1.00 | 0.87 |
| ho029 | FAST_LAN | uniform_10mb_spatial | count_by | 0.02 | warm | duckdb | duckdb | 90 | 157 | measured | yes | 1.00 | 0.66 |
| ho030 | FAST_LAN | uniform_10mb_shuffled | features | 0.0005 | cold | duckdb | duckdb | 98 | 709 | measured | yes | 1.00 | 0.87 |
| ho031 | FAST_LAN | clustered_10mb_shuffled | features | 0.1 | warm | duckdb | duckdb | 147 | 364 | measured | yes | 1.00 | 0.95 |
| ho032 | FAST_LAN | uniform_100mb_spatial | count_by | 0.1 | warm | duckdb | duckdb | 106 | 2092 | measured | yes | 1.00 | 0.75 |
| ho033 | FAST_LAN | uniform_100mb_shuffled | count_by | 0.1 | cold | duckdb | duckdb | 107 | 4307 | measured | yes | 1.00 | 0.75 |
| ho034 | FAST_LAN | clustered_100mb_shuffled | count | 0.1 | warm | duckdb | duckdb | 108 | 3754 | measured | yes | 1.00 | 0.95 |
| ho035 | FAST_LAN | clustered_30mb_spatial | features | 0.0005 | cold | duckdb | duckdb | 90 | 864 | measured | yes | 1.00 | 0.87 |
| ho036 | FAST_LAN | clustered_30mb_spatial | features | 0.02 | cold | duckdb | duckdb | 102 | 922 | measured | yes | 1.00 | 0.87 |
| ho037 | FAST_LAN | clustered_30mb_shuffled | features | 0.02 | cold | duckdb | duckdb | 119 | 956 | measured | yes | 1.00 | 0.95 |
| ho038 | FAST_LAN | uniform_30mb_spatial | count_by | 0.5 | warm | duckdb | duckdb | 92 | 347 | measured | yes | 1.00 | 0.75 |
| ho039 | FAST_LAN | uniform_30mb_shuffled | features | 0.0005 | cold | duckdb | duckdb | 178 | 973 | measured | yes | 1.00 | 0.87 |
| ho040 | FAST_LAN | uniform_10mb_spatial | count | 0.5 | cold | duckdb | duckdb | 92 | 678 | measured | yes | 1.00 | 0.95 |
| ho041 | FAST_LAN | uniform_10mb_shuffled | features | 0.5 | warm | duckdb | duckdb | 105 | 196 | measured | yes | 1.00 | 0.92 |
| ho042 | GOOD_BROADBAND | clustered_10mb_shuffled | features | 0.1 | cold | duckdb | duckdb | 185 | 1233 | measured | yes | 1.00 | 0.93 |
| ho043 | GOOD_BROADBAND | uniform_100mb_spatial | features | 0.0005 | warm | duckdb | duckdb | 137 | 1787 | measured | yes | 1.00 | 0.86 |
| ho044 | GOOD_BROADBAND | uniform_100mb_shuffled | count_by | 0.0005 | warm | duckdb | duckdb | 176 | 9477 | measured | yes | 1.00 | 0.86 |
| ho045 | GOOD_BROADBAND | clustered_100mb_shuffled | features | 0.0005 | cold | duckdb | duckdb | 137 | 10111 | measured | yes | 1.00 | 0.86 |
| ho046 | GOOD_BROADBAND | clustered_30mb_spatial | count_by | 0.0005 | warm | duckdb | duckdb | 105 | 1303 | measured | yes | 1.00 | 0.86 |
| ho047 | GOOD_BROADBAND | clustered_30mb_spatial | features | 0.02 | warm | duckdb | duckdb | 160 | 1499 | measured | yes | 1.00 | 0.86 |
| ho048 | GOOD_BROADBAND | clustered_30mb_shuffled | count_by | 0.0005 | warm | duckdb | duckdb | 106 | 1365 | measured | yes | 1.00 | 0.75 |
| ho049 | GOOD_BROADBAND | uniform_30mb_spatial | features | 0.0005 | warm | duckdb | duckdb | 129 | 1355 | measured | yes | 1.00 | 0.86 |
| ho050 | GOOD_BROADBAND | uniform_30mb_shuffled | features | 0.02 | warm | duckdb | duckdb | 392 | 2117 | measured | yes | 1.00 | 0.74 |
| ho051 | GOOD_BROADBAND | uniform_10mb_spatial | features | 0.0005 | warm | duckdb | duckdb | 115 | 510 | measured | yes | 1.00 | 0.86 |
| ho052 | GOOD_BROADBAND | uniform_10mb_shuffled | count_by | 0.5 | cold | duckdb | duckdb | 105 | 1017 | measured | yes | 1.00 | 0.75 |
| ho053 | GOOD_BROADBAND | clustered_10mb_shuffled | features | 0.5 | warm | duckdb | duckdb | 134 | 567 | measured | yes | 1.00 | 0.94 |
| ho054 | GOOD_BROADBAND | uniform_100mb_spatial | features | 0.1 | warm | duckdb | duckdb | 679 | 4331 | measured | yes | 1.00 | 0.93 |
| ho055 | GOOD_BROADBAND | uniform_100mb_shuffled | count_by | 0.5 | cold | duckdb | duckdb | 123 | 10072 | measured | yes | 1.00 | 0.75 |
| ho056 | GOOD_BROADBAND | clustered_100mb_shuffled | count_by | 0.5 | warm | duckdb | duckdb | 134 | 9474 | measured | yes | 1.00 | 0.75 |
| ho057 | GOOD_BROADBAND | clustered_30mb_spatial | count | 0.1 | warm | duckdb | duckdb | 105 | 1312 | measured | yes | 1.00 | 0.94 |
| ho058 | GOOD_BROADBAND | clustered_30mb_spatial | count_by | 0.0005 | warm | duckdb | duckdb | 107 | 1305 | measured | yes | 1.00 | 0.86 |
| ho059 | GOOD_BROADBAND | clustered_30mb_shuffled | count_by | 0.1 | warm | duckdb | duckdb | 121 | 1380 | measured | yes | 1.00 | 0.75 |
| ho060 | GOOD_BROADBAND | uniform_30mb_spatial | count | 0.02 | warm | duckdb | duckdb | 106 | 1327 | measured | yes | 1.00 | 0.94 |
| ho061 | GOOD_BROADBAND | uniform_30mb_shuffled | features | 0.02 | warm | duckdb | duckdb | 178 | 1538 | measured | yes | 1.00 | 0.93 |
| ho062 | MOBILE | clustered_100mb_shuffled | features | 0.02 | warm | duckdb | duckdb | 493 | 23531 | measured | yes | 1.00 | 0.91 |
| ho063 | MOBILE | clustered_30mb_spatial | count | 0.0005 | warm | duckdb | duckdb | 169 | 8165 | measured | yes | 1.00 | 0.92 |
| ho064 | MOBILE | clustered_30mb_spatial | count_by | 0.02 | cold | duckdb | duckdb | 166 | 8692 | measured | yes | 1.00 | 0.84 |
| ho065 | MOBILE | clustered_30mb_shuffled | count | 0.02 | warm | duckdb | duckdb | 164 | 8372 | measured | yes | 1.00 | 0.92 |
| ho066 | MOBILE | uniform_30mb_spatial | features | 0.02 | cold | duckdb | duckdb | 220 | 8819 | measured | yes | 1.00 | 0.92 |
| ho067 | MOBILE | uniform_30mb_shuffled | features | 0.0005 | warm | duckdb | duckdb | 223 | 8479 | measured | yes | 1.00 | 0.84 |
| ho068 | MOBILE | uniform_10mb_spatial | features | 0.5 | warm | duckdb | duckdb | 382 | 3046 | measured | yes | 1.00 | 0.91 |
| ho069 | MOBILE | uniform_10mb_shuffled | features | 0.5 | warm | duckdb | duckdb | 2212 | 4710 | measured | yes | 1.00 | 0.90 |
| ho070 | MOBILE | clustered_10mb_shuffled | count_by | 0.0005 | warm | duckdb | duckdb | 166 | 2913 | measured | yes | 1.00 | 0.84 |
| ho071 | MOBILE | uniform_100mb_spatial | features | 0.5 | warm | duckdb | duckdb | 401 | 4170 | measured | yes | 1.00 | 0.91 |
| ho072 | MOBILE | uniform_100mb_shuffled | count_by | 0.02 | cold | duckdb | duckdb | 181 | None | error | yes | 1.00 | 0.73 |
| ho073 | MOBILE | clustered_100mb_shuffled | count_by | 0.1 | warm | duckdb | duckdb | 184 | None | error | yes | 1.00 | 0.73 |
| ho074 | MOBILE | uniform_1gb_spatial | count_by | 0.0005 | warm | duckdb | duckdb | 178 | 16853 | measured | yes | 1.00 | 0.84 |
| ho075 | MOBILE | clustered_30mb_spatial | count_by | 0.5 | warm | duckdb | duckdb | 167 | 8178 | measured | yes | 1.00 | 0.73 |
| ho076 | MOBILE | clustered_30mb_shuffled | count_by | 0.02 | warm | duckdb | duckdb | 178 | 8371 | measured | yes | 1.00 | 0.73 |
| ho077 | MOBILE | uniform_30mb_spatial | features | 0.0005 | cold | duckdb | duckdb | 194 | 8791 | measured | yes | 1.00 | 0.84 |
| ho078 | MOBILE | uniform_30mb_shuffled | features | 0.02 | cold | duckdb | duckdb | 1045 | 9675 | measured | yes | 1.00 | 0.72 |
| ho079 | MOBILE | uniform_10mb_spatial | features | 0.0005 | warm | duckdb | duckdb | 176 | 2872 | measured | yes | 1.00 | 0.84 |
| ho080 | MOBILE | uniform_10mb_shuffled | features | 0.02 | warm | duckdb | duckdb | 236 | 2949 | measured | yes | 1.00 | 0.92 |
| ho081 | MOBILE | clustered_10mb_shuffled | count_by | 0.02 | warm | duckdb | duckdb | 166 | 2927 | measured | yes | 1.00 | 0.73 |
| ho082 | HIGH_LATENCY | clustered_30mb_shuffled | count | 0.02 | warm | duckdb | duckdb | 258 | 12632 | measured | yes | 1.00 | 0.91 |
| ho083 | HIGH_LATENCY | uniform_30mb_spatial | count_by | 0.02 | warm | duckdb | duckdb | 247 | 12359 | measured | yes | 1.00 | 0.72 |
| ho084 | HIGH_LATENCY | uniform_30mb_shuffled | features | 0.1 | warm | duckdb | duckdb | 679 | 12829 | measured | yes | 1.00 | 0.90 |
| ho085 | HIGH_LATENCY | uniform_10mb_spatial | count_by | 0.02 | warm | duckdb | duckdb | 249 | 4363 | measured | yes | 1.00 | 0.72 |
| ho086 | HIGH_LATENCY | uniform_10mb_shuffled | features | 0.5 | warm | duckdb | duckdb | 3156 | 6332 | measured | yes | 1.00 | 0.89 |
| ho087 | HIGH_LATENCY | clustered_10mb_shuffled | count_by | 0.0005 | warm | duckdb | duckdb | 266 | 4412 | measured | yes | 1.00 | 0.83 |
| ho088 | HIGH_LATENCY | uniform_100mb_spatial | count | 0.02 | cold | duckdb | duckdb | 247 | 14416 | measured | yes | 1.00 | 0.91 |
| ho089 | HIGH_LATENCY | uniform_100mb_shuffled | features | 0.5 | warm | duckdb | duckdb | 568 | 4899 | measured | yes | 1.00 | 0.90 |
| ho090 | HIGH_LATENCY | clustered_100mb_shuffled | count_by | 0.0005 | cold | duckdb | duckdb | 272 | None | error | yes | 1.00 | 0.83 |
| ho091 | HIGH_LATENCY | clustered_30mb_spatial | features | 0.02 | warm | duckdb | duckdb | 337 | 12303 | measured | yes | 1.00 | 0.83 |
| ho092 | HIGH_LATENCY | clustered_30mb_spatial | features | 0.1 | warm | duckdb | duckdb | 3171 | 14133 | measured | yes | 1.00 | 0.71 |
| ho093 | HIGH_LATENCY | clustered_30mb_shuffled | features | 0.5 | warm | duckdb | duckdb | 567 | 12778 | measured | yes | 1.00 | 0.90 |
| ho094 | HIGH_LATENCY | uniform_30mb_spatial | features | 0.0005 | cold | duckdb | duckdb | 284 | 12945 | measured | yes | 1.00 | 0.83 |
| ho095 | HIGH_LATENCY | uniform_30mb_shuffled | features | 0.0005 | warm | duckdb | duckdb | 308 | 12692 | measured | yes | 1.00 | 0.83 |
| ho096 | HIGH_LATENCY | uniform_10mb_spatial | features | 0.5 | warm | duckdb | duckdb | 327 | 4381 | measured | yes | 1.00 | 0.90 |
| ho097 | HIGH_LATENCY | uniform_10mb_shuffled | features | 0.5 | warm | duckdb | duckdb | 586 | 4599 | measured | yes | 1.00 | 0.90 |
| ho098 | HIGH_LATENCY | clustered_10mb_shuffled | features | 0.0005 | warm | duckdb | duckdb | 246 | 4405 | measured | yes | 1.00 | 0.83 |
| ho099 | HIGH_LATENCY | uniform_100mb_spatial | features | 0.5 | warm | duckdb | duckdb | 355 | 6503 | measured | yes | 1.00 | 0.91 |

</details>

Reading: with the final design the browser wins only on LOCAL, only for small files and small results (4 of 100). Everywhere else the planner correctly picks the server — and so does a rule that always picks the server (96 %). What the planner adds is the 2 LOCAL cases it misses are near-ties (regret ≤ 1.08) and — more important — that it keeps the browser out of cases where choosing it would have cost 10–50×. Always-browser has median regret 9.5× and P90 49×.

## 10. Confidence Model

Weighted geometric mean (estimate quality 0.25, covering 0.10, probe freshness 0.15, health 0.15, network uncertainty 0.15, dataset familiarity 0.10, operation certainty 0.10) × margin factor (0.6 coin flip … 1.0 clear win); every factor returned with value and reason, visible in the UI. Observed: no case below 0.5, 1 in 0.50–0.65, 35 in 0.65–0.80 (94 % correct), 64 ≥ 0.80 (100 % correct). Direction is right; the sample is too small and too server-heavy to call the score calibrated. A confidence-aware policy exists only for the browser branch (R5b) — in this data it never overturned a correct choice, and it rarely had occasion to act.

## 11. Failure / Fallback

Classes: CAPABILITY, MEMORY, TIMEOUT, CANCELLED, ENGINE_CRASH, UNSUPPORTED_FORMAT, UNSUPPORTED_OPERATION, TRANSIENT (Rust and TypeScript classifiers agree on 18 shared fixture messages). Real-browser test on this build: a simulated crash falls back once to the server with class ENGINE_CRASH, invalidates and re-probes the runtime; a deterministic format failure falls back once and is remembered (the planner will not offer that engine for that source again; another source is unaffected); a cancellation does **not** fall back; the retry function is called exactly once. Same logical plan, reason shown in the UI. Not done: server → browser fallback (not asked for), staged plans.

## 12. Browser Stability

### Browser stability (held-out)

browser runs 96 · crashes 0 · engine errors 3 (30 s timeouts on slow links, planner had chosen the server) · skipped above the tab limit 1 · digest mismatches 0 of 29 checked · row-count mismatches 0 · browser chosen and then failed: 0

The 3 engine errors are 30 s timeouts of the browser engine on 100 MB shuffled files over MOBILE/HIGH_LATENCY, measured with an engine hint (the planner had chosen the server in all three; the timeout is the engine's own protection). No tab crash occurred in the held-out set (96 browser runs), in the calibration re-runs (133 runs) or in the e2e suite on this build. The crash threshold used by the risk score is **not** a measurement of the new design (see limits). Crash behaviour above ~150 MB of fetched bytes was not probed.

## 13. UI

Execution Plan panel (screenshot `docs/screenshot-planner-hardening.png`): Engine (+Local/Server), Confidence (badge High/Medium/**Low Confidence** + engine × margin, factor list on hover), Health per engine (Healthy/Degraded/Unhealthy/Unknown), Runtime probe (Observed/Stale/Not probed/Failed, age, memory class, wasm version, failed checks), Network profile (+origin), Result estimate (**Estimated** badge, interval, quality) next to **Observed** actual rows and the error factor, Browser risk (score, memory vs the crash threshold), Fallback (planned, max 1, no retry; occurred, with class), policy notes. Settings: assumed network profile. Verified in Chrome by 7 tests. Known UX defect: the panel sits below the (long) reasons list; on a small window it needs scrolling.

## 14. Regression

| Suite | Result |
|---|---|
| Rust (`cargo test --workspace --no-default-features`) | **137 passed**, 0 failed (was 98): planner lib + integration, 12 hardening, selection matrix, 12 gateway, source/estimator inputs, IR/DAG, engine, join, timeout |
| TypeScript unit (`node --test`) | **23 passed** (was 17): sqlgen, digest, wkb, failure (shared fixture), probe model |
| Browser e2e (real Chrome) | **17 passed** (was 9): smoke 1, engines 9 (incl. new real-Overture browser = server digest test), planner 7 |
| Architecture fitness | `arch_check.py` OK |
| Cross-engine digest equality | held-out: 29 deterministic cases equal, 0 mismatches; Overture count_by and features equal in Chrome |
| Server benchmarks vs independent oracle | Overture **7/7 pass**; synthetic 10 MB + 100 MB **56 pass**, 0 validation failures |
| IR migration, DuckDB/WASM correctness, gateway | covered by the above (IR/DAG tests unchanged and green) |
| Not re-run | 1 GB synthetic oracle suite (server code path unchanged this phase); bundled-DuckDB build (still only the prebuilt path is exercised) |

## 15. Architecture Debt

1. **Same DuckDB family on both sides.** Replaceability across engine families is still unproven; the planner's contract has not been exercised by a different system.
2. **Duplicated lowering (TS vs Rust).** It already produced a defect this phase (typed geometry) and a second class of bugs would be silent. RFC-004 (proposal only): conformance fixtures first, a shared template table when a third lowering appears.
3. **No staged plans.** One engine per plan; `stages` is still a single-element list.
4. **No PostGIS** (by instruction). **Simulated network** only. **One machine/browser**; memory threshold not measured for the new access pattern.
5. Estimator: corrections do not generalise across data distributions (section 6).

## 16. ADR Status

- ADR-009 (runtime capability probing): **Proposed**. ADR-010 (confidence, health, failure taxonomy, network): **Proposed**. RFC-004 (canonical lowering): **Proposed**, research only.
- ADR-008: **stays Proposed** with a dated correction (two of its stated facts were wrong). ADR-003, ADR-007: unchanged (Accepted). No status was promoted.

## 17. Remaining P0 / Recommendation

P0 before relying on the planner beyond this machine:
1. **Estimator tail** (P90 11.5× vs target 5×): do not apply a calibration learned on one data distribution to another without a guard; do not mark a LIMIT-bound estimate "good" when coherence is low. Needs a fresh held-out set, not this one.
2. **Real network validation**: Chrome-level throttling (CDP) or a second machine; the simulation shares my assumptions about how a link behaves.
3. **Browser memory model**: measure the crash point for the *range* access pattern between 100 MB and 1 GB of fetched bytes; today's 245 MB is a legacy figure.
4. A second browser/device through the probe (Firefox/Safari have no `deviceMemory`/heap APIs).

P1: lowering conformance fixtures (RFC-004 option D); the UI panel layout; telemetry persistence policy (it grows without bound on disk).

**Recommendation: KEEP HARDENING PLANNER.** The machinery asked for exists and behaves (probe, health, fallback, confidence, network terms, visibility; accuracy and regret targets met; no crash, no wrong result). It is not READY FOR POSTGIS ENGINE because (i) the estimator misses its tail target, (ii) the headline accuracy is not better than a constant rule on this machine, so the branch that would justify a smarter planner (several real engine choices with real trade-offs) has not been exercised — a PostGIS engine would be the first real test, but it should be added to a planner whose estimates are trustworthy, and (iii) the network evidence is simulated. It is not REWORK: the architecture (descriptor/executor split, `PlanInputs`, assessment) held up, and the two big defects were found by the new machinery and fixed without touching the IR.

## Appendix — files

Evidence: `benchmarks/results/heldout-*.json` (per network), `heldout-cases.json`, `heldout-summary.json`, `summary-planner-hardening.md` (full per-case tables), `calibration-set.jsonl`, `cal2-*.json`, `bench-hardening-{overture,small}.md`, `exploratory-ho1/`. Reproduce: `python benchmarks/run_heldout.py LOCAL FAST_LAN GOOD_BROADBAND MOBILE HIGH_LATENCY` (env `HO_SAFE_MB=150`), then `analyze_heldout.py --uncalibrated <gateway without telemetry>` and `make_report_tables.py`.
