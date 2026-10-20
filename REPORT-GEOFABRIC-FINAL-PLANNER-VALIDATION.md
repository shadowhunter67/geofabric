# REPORT — GeoFabric final planner validation (2026-10-19/20)

Written for: the Principal Architect deciding whether PostGIS may start. Nothing of PostGIS was started; no IR, metric-CRS, SemiJoin, UI-architecture, data-pack or raster semantics were changed.

## Executive Summary

| Item | Result |
|---|---|
| Timing contract | Defined and enforced: one clock inside the page; the harness, the model and the report use the same quantity. |
| LOCAL/FAST_LAN bias | **Partly explained, not removed.** It is common-mode (the server model is under-predicted by the same factor on LOCAL), so decisions are preserved (every wrong choice cost ≤ 75 ms). The model was NOT adjusted after seeing the held-out. |
| Fresh held-out (seed 20261026, new data draw, 200 cases incl. 45 above 128 MiB) | browser model **median 1.23× (gate 1.25×), P90 2.66× (gate 3×)**; planner accuracy 95.0 % (always-server 81.5 %); **regret P90 1.00, max 1.59 (75 ms)**; **0 unsafe choices, 0 crashes, 0 digest/row mismatches.** |
| R5c (no automatic browser above 128 MiB) | Validated on 45 large cases: never selected the browser; cost of the policy ≤ 75 ms in the 3 cases where the browser was faster; saved up to 1.6× in the 5 where it was slower. |
| Independent physical device | **NOT done: no second physical machine was available** (asked; answer: none). Chrome, Edge, Firefox and WebKit all ran the device matrix on this one machine. |
| Regression | Rust 190/190, TypeScript 76/76 + typecheck, arch_check OK, Demo B oracle identical (27,624), workflows A–E and rendering specs pass on Chrome and Edge. |
| Flake | **20 combined iterations (14 Chrome + 6 Edge), 60 tests each = 1,200 executions, 0 failures** after fixing three test races found on the way. |
| ADR | ADR-008 → **Accepted, scoped**. ADR-010 → **stays Proposed** (needs the second device). |

**PostGIS readiness: KEEP HARDENING PLANNER.** The only gate item that cannot be closed from this machine is the independent physical device; the LOCAL/FAST_LAN bias is documented but unresolved. Everything else of the brief passes. Closing the device item needs one laptop and about 10 minutes (`docs/claude/second-device-check.md`).

## Timing Contract

Browser time = **one clock inside the page** (`performance.now()`), from the moment the placed request leaves to the moment the result exists in the tab:

```
plan request + response (gateway planning included)
+ engine start (cold only: engine CPU + assets over the link + first-query warm-up)
+ source open + first batch + drain + conversion to the server-shaped result
```
Excluded: the test driver's round trips and everything after the result (drawing). Server time = the same POST through the page, wall clock, same link. Implemented in `web/e2e/heldout.spec.ts` (`out.total`), documented at the top of `crates/geo-planner/src/browser_model.rs`, and the phases (`init/open/first/drain/boundary`) come from the engine itself.

Why it mattered: the previous harness clock ran in the test driver and included ~3 Playwright round trips (the model's `plan_ms` had absorbed them: 7.0 ms before, 3.4 ms now); an older version also serialised the whole result through the driver. The old definition is still recorded (`outer_ms`) for comparison. Because the contract changed, the model was **recalibrated** (159 runs, `ccal-*`) and refitted before the new fresh held-out.

## LOCAL / FAST_LAN Root Cause

Observed: browser time under-predicted on the fast profiles, in two independent fresh held-outs (LOCAL median 1.74× before, 1.78× now; FAST_LAN 1.79×, 1.41×), against 1.0–1.13× on GOOD_BROADBAND/MOBILE/HIGH_LATENCY. Evidence gathered (nothing refitted on held-out data):

1. **Environment variance is large.** The identical calibration and held-out cases re-measured hours later (diagnostic re-run, 106 paired warm cases): browser now/then P10–P90 = 0.56–1.68×, server 0.61–1.83×; log drifts correlate 0.79 between the engines; browser relative to server only 0.70–1.18×. A global multiplier would just fit one machine state.
2. **Common-mode on LOCAL.** In the final run the server model is under-predicted by 1.63× on LOCAL and 1.67× on FAST_LAN, the browser by 1.67× and 1.42×: both engines shift together, so the *ratio* the planner uses is preserved. Consistent with the outcome: LOCAL's wrong choices are all near ties (≤ 75 ms).
3. **Quantities are not the cause.** Predicted vs measured range requests: median 0.96–1.00 (features), 0.78–1.01 (aggregates); bytes median 0.95–1.12.
4. **Structural gaps found (small).** Range mode prices bytes only at link speed — no decode-per-MB term (whole-file mode has one). Post-hoc diagnostic only (fit on calibration, evaluate on the final run, using measured requests/bytes): adding the term reduces the range first-batch error on LOCAL+FAST_LAN from 1.49× to 1.30× (13 cases). The whole-file open cost per MB is ~20 % low at 30 MB (measured 1.36 ms/MB vs 1.13 ms/MB in calibration, whose files were mostly 10 MB).
5. **Drift is not the whole story.** A fixed reference query run on both engines at the start of every dataset batch varies 2× on LOCAL within one run (browser 37–73 ms, server 77–153 ms) and ~1 % on MOBILE/HIGH_LATENCY (network-bound), but it does not predict the browser error (correlation 0.16; normalising by it does not help).

Conclusion: no single cause. Host-level performance variance on CPU-bound (fast-link) cases plus two small structural omissions. Because the model gate passed and the shift is common-mode, the model was **not changed**; the decode term is listed as the next change (it needs a new calibration and a new fresh held-out — changing it now would contaminate this one). The planner stays conservative by construction (R5b/R5c).

## Browser Cost Model

Sum of measured parts (`crates/geo-planner/src/browser_model.rs`), constants fitted by NNLS on `ccal-*` only, frozen at commit `e909764` before the final data existed:

| part | formula (ms) |
|---|---|
| plan round trip | 3.4 + 1.14·RTT + 19.1·[throttled] |
| cold start | 416 + 0.95·wasm(36.7 MB)/link + 116·[thr] + 199 |
| whole-file open | 7.6 + 1.08·file/link + 39.7·[thr] |
| whole-file first query | 4.8 + 0.66·kept MB + 6.8·result-geometry MB |
| range first query | 7.1 + n·(1.9 + 0.93·RTT + 26.0·[thr]) + 1.04·bytes/link |
| result | 0.79 + 4.2/krow + 4.9/geometry-MB + 18.8/attribute-MB |

Requests and bytes of range mode come from planner-known features (row groups kept, covering bytes, LIMIT-bound or not) — unchanged. Geometry bytes of the result are exact for sources with the row index (≤ 100,000 rows), else the mean row size; aggregates carry no geometry.

## Server Model

Problem found: the server result term ignored geometry, so 25 administrative-boundary rows were predicted at 84 ms and took 818 ms (three of the six wrong choices of the previous gate were this). Now: `server = 83 + 0.2·input MB + 155·geometry MB + 234·attribute MB (+ network)` (the geometry/attribute rates were fitted on the 22 LOCAL feature queries of the calibration set: median error 1.09×, P90 1.23×), result bytes on the wire = GeoJSON text (3× WKB, 100 B/row floor). Calibration (Rust planner): median 1.12×, P90 1.71×, max 2.3× (admin layer 1.20×). **Fresh held-out: median 1.40×, P90 2.02×, P95 2.26×, max 3.68×** (≤ 128 MiB: 1.37×/1.90×; > 128 MiB: 1.69×/2.41×; real pack layers 1.43×/1.92×). The server is under-predicted on this machine's fast profiles by the same common-mode factor as the browser; the **planner harm is nil in this run** (no wrong choice from the server side beyond 75 ms), so the remaining error is a P1.

## R5c Policy

`source file larger than 128 MiB (the largest file the browser model was calibrated on) → the browser is never chosen automatically; an explicit engine choice still works`. Cost model and policy stay separate: the candidate scores are unchanged, the override is written to `assessment.policy` ("R5c outside the calibrated range: … {engine} is {x}% cheaper on paper only; {server} chosen"). No dataset names appear anywhere in planner logic (size, layout, capability, health, device, network and operation only).

## Large-file Validation

New data draw `_s6` (seed 20261026): 300 MB ×4 (clustered/uniform × spatial/shuffled), 500 MB ×2, 1 GB ×2 (synthetic) plus the real 1.45 GB Overture buildings with new windows; 45 of the 200 cases (LOCAL 34 %, FAST_LAN 30 %, GOOD 22 %, MOBILE 12 %, HIGH_LATENCY 8 % of their cases — slow links get few because a 1 GB range query at 150 ms RTT takes minutes).

| | |
|---|---|
| large cases | 45 (32 browser-measured; 13 not runnable in the browser: fetch estimate above the 220 MB tab threshold → the server is the safe winner) |
| cost model alone would have chosen the browser | 8 |
| R5c fired | 8 (exactly those) |
| browser selected by the planner | **0** |
| policy decision correct | 41/45 (91.1 %) |
| regret (median / P90 / P95 / max) | 1.00 / 1.00 / 1.13 / 1.59 |
| model error on the 8 R5c cases | 4–6× (predicted 20–80 ms, measured 128–430 ms) |

The 8 R5c cases (server ms vs browser ms): f004 403/429, f006 225/247, f024 **219/153**, f026 143/233, f045 152/186, f009 **328/314**, f047 **202/128**, f023 93/124 — the browser was faster in 3 (saved ≤ 75 ms), slower in 5 (by 1.07–1.63×). Largest cost of the policy: 75 ms on a 128 ms query. One large case outside R5c (f063: model said server, browser measured 180 vs 208 ms) was also a ≤ 28 ms difference. Details per case: `benchmarks/results/final-summary.json` (`large_files.table`: model scores, model-fastest, policy-selected, actual-fastest, R5c, regret).

## Calibration

`ccal-*` (159 runs, seed 20261020, never used as held-out; synthetic draws `_s2/_s3`, 1 GB shuffled, the four Overture pack layers; 5 profiles by Chrome CDP; 30 % cold). In-sample with the Rust planner: browser median 1.07×, P90 1.67×, P95 1.83×, max 7.5× (LOCAL 1.16/1.78, FAST_LAN 1.28/1.88, GOOD 1.06/1.38, MOBILE 1.03/1.40, HIGH_LATENCY 1.03/1.32); with measured instead of predicted requests/bytes 1.05×/1.52×; leave-one-dataset-family-out 1.06×/1.76×. Server: 1.12×/1.71×.

## Fresh Held-out

Seed 20261026 (20261019 not reused), new `_s6` data draw, 200 cases: LOCAL 56, FAST_LAN 36, GOOD_BROADBAND 36, MOBILE 36, HIGH_LATENCY 36; 118 synthetic ≤ 100 MB, 37 real pack layers (same files as calibration, new windows), 45 above 128 MiB. 53 cold by the engine's own state. Model, policy and estimator frozen beforehand (commit `e909764`), `--no-learn`, gateway port checked free and profile verified per run, run once; no harness bug appeared, so no re-seed. Raw: `benchmarks/results/final-*.json`, analysis `final-summary.json`.

Browser time model (187 measurable cases):

| slice | n | median | P90 | P95 | max |
|---|---|---|---|---|---|
| **all** | 187 | **1.23×** | **2.66×** | 3.67× | 6.6× |
| ≤ 128 MiB sources | 155 | 1.16 | 2.09 | 2.58 | 3.8 |
| > 128 MiB (R5c domain, no claim) | 32 | 1.74 | 5.53 | 6.07 | 6.6 |
| LOCAL | 52 | 1.78 | 3.79 | 5.31 | 6.2 |
| FAST_LAN | 33 | 1.41 | 2.14 | 2.64 | 3.8 |
| GOOD_BROADBAND | 33 | 1.10 | 1.65 | 1.93 | 2.5 |
| MOBILE | 34 | 1.06 | 1.46 | 3.14 | 6.6 |
| HIGH_LATENCY | 35 | 1.06 | 1.40 | 2.38 | 4.6 |
| whole-file / range | 109 / 78 | 1.06 / 1.43 | 1.85 / 3.74 | 2.21 / 5.17 | 3.8 / 6.6 |
| warm / cold | 139 / 48 | 1.25 / 1.18 | 3.19 / 1.63 | 3.78 / 1.77 | 6.2 / 6.6 |

Honest reading: both overall gates pass (the median by 0.02, so a different machine state could flip it); LOCAL fails its own P90 and the range-mode P90 is 3.7× — the gate is met in aggregate, not in every slice. The cardinality estimator was not re-evaluated in this phase (unchanged since the previous gate, where it measured median 1.00× / P90 1.19× with exact admin-layer cardinality).

## Planner Accuracy

200 cases, both engines measured wherever the browser could run safely: **95.0 %** correct (always-server 81.5 %; the cost model alone 94.5 %). ≤ 128 MiB 96.1 %, > 128 MiB 91.1 %; warm 93.2 %, cold 100 %. By profile: LOCAL 89.3 %, FAST_LAN 91.7 %, GOOD_BROADBAND 97.2 %, MOBILE 100 %, HIGH_LATENCY 100 %. Browser chosen in 33 cases, actually fastest in 37.

## Planner Regret

Regret = time(selected)/time(fastest): **P50 1.00, P90 1.00, P95 1.001, max 1.59**. Gate P90 ≤ 1.25: met (≤ 128 MiB sources: P90 1.00, max 1.38). Absolute penalty: P90 0 ms, **max 74.9 ms**, sum over all 10 wrong choices 348 ms. The 10 wrong choices, all on LOCAL/FAST_LAN/GOOD with queries of 93–400 ms: f047 (+75 ms, R5c, browser 128 vs server 202), f024 (+66 ms, R5c), f051 (+60 ms, browser 217 vs 157), f110 (+44 ms), f087 (+33 ms), f063 (+28 ms), f017 (+21 ms), f009 (+14 ms, R5c), f005 (+5 ms), f062 (+2 ms). Nothing like the previous gate's 2.3 s vs 0.5 s admin mistake (that one is gone: server model fixed). Three of the ten are R5c's price.

## Independent Physical Device

**Not satisfied.** The brief requires a second physical machine; none was available (asked; the answer was "no second machine"). What exists:

| browser (engine) | where | result |
|---|---|---|
| Chrome 155 (Blink) | this machine | 7/7 runnable cases: browser digest = server digest; unsafe candidate kept on the server |
| Edge 155 (Blink) | this machine | same |
| Firefox 157 (Gecko, installed 2026-10-19 via Playwright) | this machine | same: DuckDB-Wasm works in Firefox for this matrix |
| WebKit (Playwright build, Windows) | this machine | same |

All four on the same CPU/RAM/OS, so they are evidence about engine independence of the *correctness* path, not about hardware (the matrix JSON records cores 28 / memory class 32 GB on this device). Timing from other hardware is unmeasured. To close it: `docs/claude/second-device-check.md` (open `http://<LAN IP>:8787/?devicecheck=1` on another laptop; needs your approval to expose the gateway on the LAN IP; the report is saved by the server). I did not open any port.

## Firefox / Alternate Browser

Firefox 157 and WebKit: the 8-case device matrix completed in both (about a minute each); 7 runnable cases matched the server's result digest; the unsafe candidate (1.45 GB, 100,000-row result) was kept on the server; no tab crash. Firefox/WebKit are not in the Playwright suite (flake loop) — the suite runs Chrome and Edge. Supplemental, as specified.

## Browser Safety

0 unsafe browser selections, 0 browser crashes, 0 digest mismatches, 0 row mismatches in 200 cases; 13 cases correctly never ran in the browser (fetch estimate above the tab threshold); the matrix's unsafe candidate stayed on the server in all four browsers; the 220 MB tab threshold and the 150 MB result envelope were not relaxed; R5c adds a second, independent barrier above 128 MiB.

## Metric Spatial Regression

Unchanged implementation. `cargo test` metric/UTM tests (`crates/geo-types/src/metric.rs`, `crates/geo-ir/tests/semi_join.rs`) pass; Demo B oracle re-run: GeoFabric ids == pyproj + shapely oracle ids (27,624 = 27,624, same sha256), 3 of 1,955 near-limit pairs classify differently from true geodesic distance, 1.6 s (`oracle-near-final.json`).

## SpatialSemiJoin Regression

`crates/geo-runtime/tests/semi_join.rs` (six correctness cases, geodesically placed points at 199/201 m), `crates/geo-ir/tests/semi_join.rs` (validation, serialisation, schema), gateway `near`/`near_count` tests: all pass in the 190-test Rust run. The browser engine keeps refusing `SpatialSemiJoin`/`Transform` as unsupported (golden fixtures), so Demo B is server-only.

## Representative Rendering Regression

Four-cluster test (`templates.rs`: the file-order cut shows one corner, the thinned answer all four; the sample of a filtered query is a subset of the filtered result), sampling unit tests, and `sampling.spec.ts` on real buildings (3/3 on Chrome and Edge: quadrant shares within 4 points of the truth, deterministic digest, server and browser draw the same sample). Sampling affects only rendering: counts/aggregates stay full-result, an aggregate with `sample_modulus` is refused (HTTP 400).

## Real-data Workflows

`data-pack.spec.ts`: A buildings in view, B within 200 m of main roads (metric Transform + SpatialSemiJoin), C buildings per ward/commune, D schools/hospitals in view, E terrain overlay — Chrome 15/15, Edge 15/15 (a Chrome failure of the "click a road, a place, a ward" test was found and fixed: see Flake Validation).

## Full Regression

| suite | result |
|---|---|
| Rust workspace | **190 passed, 0 failed** (incl. the new permanent `count_by` tie regression) |
| TypeScript | typecheck clean; **76 passed** |
| arch_check | OK, no dependency-boundary violations |
| Playwright Chrome / Edge | combined suite incl. data-pack + sampling: see Flake Validation |
| Demo B oracle | identical ids |
| device matrix | Chrome, Edge, Firefox, WebKit on this machine (not a second machine) |

**Permanent `count_by` regression** (brief §17): `crates/geo-gateway/tests/templates.rs::count_by_ties_are_cut_by_the_group_key_so_every_engine_returns_the_same_top_50` (120 groups all tied at 3, top 50 must be g000…g049, stable digest) + the browser golden `count_by_sorted` (`ORDER BY n DESC, category`) run by both the Rust and the TypeScript lowering tests.

## Flake Validation

`benchmarks/flake_loop.py` / `flake_run_all.sh`: the whole Playwright suite (smoke, explorer, engines, planner, map, sampling, **data-pack**: 60 tests, ~10 min per iteration) on the release gateway, port 8798 checked free before each loop, gateway PID recorded, one JSON report per iteration with start time, browser, port, PID, failures and shared state (`flake-chrome.json`, `flake-msedge.json`, `flake-run-all.log`).

**Final loop: 14 Chrome iterations (2026-10-20 12:14–14:27) + 6 Edge iterations (14:37–15:29), 0 failures.** Shared state: one gateway per iteration (health and telemetry are shared by the specs inside an iteration and reset by restart).

Failures found on the way (all fixed, evidence kept in `benchmarks/results/`):

| found by | test | symptom | cause | fix |
|---|---|---|---|---|
| earlier loops (`flake-before-fix-*`, `flake-pass2-partial-*`) | `map`: clicking a polygon selects exactly that feature | wrong feature / 0 highlighted / inspector missing (7 of 12 solo runs) | target picked from a half-drawn map (958 / 6,031 / 7,570 polygons rendered at test start) | wait for idle and a stable rendered count (20/20 solo) |
| same | `engines`: browser fails → fallback shown | `engine-fallback` never appears (≈ 25 % solo) | the runtime probe also ran in the engine; the injected failure sometimes hit the probe, the planner (rightly) never tried the browser | wait for the probe first (16/16 solo) |
| this phase's regression run (Chrome, 2 of 2 runs, then 2 of 10 solo) | `data-pack`: clicking a road, a place and a ward | inspector shows a road after clicking a place (and the reverse) | the layers load in a cascade and **refresh a second time ~1 s later**; each refresh removes and re-adds the layer, and the click landed during the swap (timeline captured: places/admin `loading` again at ~2 s) | wait until all layers are loaded and unchanged between two polls 1.2 s apart (15/15 solo) |
| earlier | `engines`: auto mode shows the server engine | selected engine `duckdb-wasm` | test encoded the old default; the planner now picks the browser on small files over a fast link (measured faster) | assert "selected + the other as alternative" |

The first 20-iteration loop of this phase excluded the data-pack spec (6 specs, 45 tests, 0 failures, archived in `flake-core-suite-20/`) and therefore did **not** cover the third race; it is superseded by the full-suite loop above. Not covered: iterations on a second physical machine; Firefox/WebKit are outside the automated suite. A product-side note from the third race: the data-pack UI refreshes every context layer twice (P1, listed under Remaining Risks).

## ADR Status

- **ADR-008** (multi-engine placement): **Accepted, scoped** (2026-10-19). Scope: one server engine + one browser engine, single-source plans; browser auto-execution validated up to 128 MiB; timing evidence from one physical machine; network emulated; replaceability across engine families (PostGIS) and multi-stage plans explicitly NOT covered.
- **ADR-010** (confidence/health/network): **remains Proposed**; final-validation addendum written. Gates met on this machine; the required second physical device is missing.
- ADR-009 (probing) Accepted (Chromium, one machine; now also Firefox/WebKit correctness); ADR-011, ADR-012 Accepted; RFC-001 unchanged in this phase.

## Remaining Risks

1. **P0 for ADR-010/PostGIS gate: independent physical device** — not run.
2. LOCAL/FAST_LAN under-prediction (common-mode, ≤ 75 ms of regret so far); the median gate passed by 0.02.
3. Range mode has no decode-per-MB term; whole-file open per MB ~20 % low at 30 MB — fix needs a recalibration and a new fresh held-out.
4. Model outside ≤ 128 MiB is wrong by 4–6× on small windows; protected by R5c only (a policy, not a model).
5. Server model median 1.40× on this host's fast profiles (P1; no decision harm observed).
6. Row index limited to sources of ≤ 100,000 rows; larger polygon layers still use the centre histogram (not measured to hurt in this run: pack cardinality was exact/near-exact).
7. The data-pack UI refreshes each layer twice (≈ 1 s apart), re-adding the layer each time; a click during the swap can pick a different layer (the test now waits for stability). P1 UX/efficiency item, not changed here (no new features).
8. Network is emulated (CDP), never a real network; Firefox/WebKit not in the automated suite.
9. Held-out reuses the four Overture pack files from calibration (new windows only) and the real 1.45 GB buildings file was measured in an earlier exploratory run (different windows, different harness).
10. Codex was used only in read-only delivery mode; all code here was written or reviewed by Claude.

## PostGIS Readiness

**KEEP HARDENING PLANNER.** Required for READY: browser timing gate ✔ (on this machine), fresh held-out ✔, R5c ✔, no unsafe choices ✔, full regression ✔ — and **second physical device ✘**. Everything the brief asked for other than that device is in place; a PostGIS phase would add a second engine *family* while the planner has only ever been timed on one machine. Smallest path to READY: run the device matrix on one other laptop (`docs/claude/second-device-check.md`); if it passes (7/7 digests, unsafe case on the server, no crash) the planner phase is complete — ADR-010 can then be accepted — and the decode-term/recalibration item can be scheduled as P1 rather than blocking.
