# ADR-010 — Planner confidence, engine health, failure taxonomy, network model

Status: **Accepted with documented scope limits** (2026-10-20, Principal Architect decision; was Proposed since 2026-10-14). The limits are part of the decision — see the last section.

**Decision.** The planner answers four questions instead of one: which engine *can* run the plan (feasibility), which *should* (score), how *confident* it is (a number with reasons), and what happens when it is wrong (bounded fallback). Inputs the planner is given besides the plan (`PlanInputs`): runtime probes (ADR-009), observed health, known incompatibilities, a calibration learned from telemetry, the network profile, whether the browser instance is warm.

## 1. Failure taxonomy

`CAPABILITY, MEMORY, TIMEOUT, CANCELLED, ENGINE_CRASH, UNSUPPORTED_FORMAT, UNSUPPORTED_OPERATION, TRANSIENT`. One classifier in Rust (`FailureClass::classify`) and one in TypeScript (`failure.ts`), both checked against the same fixture file (`crates/geo-planner/tests/fixtures/failure-classes.json`, 18 messages). The class decides: counts for health (MEMORY, TIMEOUT, ENGINE_CRASH, TRANSIENT) or not; *deterministic* (CAPABILITY, UNSUPPORTED_FORMAT, UNSUPPORTED_OPERATION → remembered per engine+source as a known incompatibility, never repeated); whether a fallback is allowed (everything except CANCELLED, which is the user's decision). **No same-engine retry exists in v1**, and at most **one** fallback (browser → server, same logical plan, reason recorded and shown in the UI).

## 2. Health

`Healthy / Degraded / Unhealthy / Unknown` per engine from the last 12 outcomes within 15 min. A crash or memory failure ⇒ Unhealthy for a 30 s cool-down, then Degraded (half-open) until the next success; 3 consecutive failures ⇒ Unhealthy; ≥ 25 % failures or a last failure ⇒ Degraded; no recent event ⇒ Unknown. Unhealthy is a hard filter, except when it would leave no engine at all (then it is ignored with a warning: a degraded answer beats none). A different engine version forgets the history.

## 3. Browser risk score (replaces the fixed memory constant)

`risk = 1 − Π(1 − component)`; components: memory (estimated bytes pulled into the tab ÷ crash threshold for the probed memory class), result rows ÷ row limit, probe state, health, unprobed file class. Pre-rejection (never traded against score): memory component > 0.6, result rows over the limit, probe failure, file class the runtime cannot read, unsupported operation, unhealthy, known incompatibility, covering column missing. The crash threshold is 245 MB at `deviceMemory = 8`, scaled linearly below — **one measurement on one machine plus an assumption**; since 2026-10-14 the browser pulls only what the query touches (ADR-009), so the number is compared with *fetched* bytes. Operator override: `--browser-max-mb` replaces the memory rejection.

## 4. Network model

Profiles LOCAL (the calibration host: loopback), FAST_LAN 5 ms/1 Gbps, GOOD_BROADBAND 20 ms/200 Mbps, MOBILE 70 ms/30 Mbps, HIGH_LATENCY 150 ms/20 Mbps (+ per-request overhead 0–5 ms). `server = compute + RTT + upload + result/BW`; `browser = compute + RTT (plan) + requests × RTT (strictly sequential: DuckDB-Wasm reads synchronously) + fetched bytes/BW`. The fitted compute terms contain no network. Profiles are presets (assumptions) unless measured; the UI shows which.

Validation: a gateway option `--simulate-network <profile>` delays every `/api` response by the profile's latency and serialises bodies on a shared-bandwidth link (token-bucket by `Content-Length`). This is a **simulation of a link between a real Chrome tab and a real gateway on one host, not a real network**; it delays the real range requests of the real DuckDB-Wasm, so request counts and sizes are measured, not modelled. (A first version of the simulation charged no bandwidth for streamed file bodies — it used `size_hint`, which streamed `ServeFile` bodies do not provide; every number from that version was discarded, see the report.) No non-loopback/Chrome-level (CDP) throttling was done.

## 5. Estimator v2

Inputs: footer statistics only (rows, row groups, bbox extents, covering quality declared/inferred/none, spatial coherence, density skew, null fraction) and a calibration. `raw` = uniform-density estimate per kept row group; `selected = min(raw × correction(source, selectivity bucket), rows of kept row groups)`; the correction is a shrunk median of past `actual / raw` (uncensored runs only — a result that hit its LIMIT only bounds the selectivity); group counts for aggregates = largest count seen for that column. Output: rows, interval, quality (good/fair/poor with reasons). It never sees the query it estimates. Telemetry: JSON lines, `--telemetry`, frozen with `--no-learn`. Error metric: `max(est+1, act+1)/min(...)`.

## 6. Confidence

Weighted geometric mean of seven factors in (0, 1]: estimate quality 0.25, covering quality 0.10, probe freshness 0.15, engine health 0.15, network uncertainty 0.15 (× the network's share of the time), dataset familiarity 0.10, operation support certainty 0.10. Plan confidence = engine confidence × margin factor (0.6 for a coin flip … 1.0 for a clear win). Every factor is returned with its value and reason. **Policy R5b** (explicit, logged): the browser is never chosen on a cost advantage when its runtime is unprobed; nor when its confidence is below 0.45; nor on a close call (< 20 % cheaper) when it is less trusted than the server. An explicit engine hint bypasses R5b (and nothing else).

## Evidence and what it does not show

See `REPORT-GEOFABRIC-PLANNER-HARDENING.md`. On the held-out set (100 cases, never used to fit or tune; 5 network profiles, warm/cold, uniform/clustered, sorted/shuffled, 4 selectivities, 8 query shapes): planner accuracy 98 %, P90 regret 1.00, **but always-server scores 96 %** — the browser is fastest in 4 of 100 cases, all on LOCAL; the accuracy claim is therefore weak evidence about the *browser* branch. Estimator target missed: median 1.12× (target < 2), **P90 11.5× (target < 5)**, max 29×; interval coverage 71 %. Time model: median 1.08× (server) / 1.09× (browser), P90 1.3× / 2.7×. Confidence is roughly ordered (accuracy 94 % in the 0.65–0.80 bin, 100 % above 0.80) but the bins are small.

**Why it stays Proposed.** One machine, one browser, one engine family on both sides (DuckDB); the estimator misses its tail target (corrections learned on clustered data hurt uniform data; a LIMIT-bound estimate is called "good" on a shuffled file where it is wrong); the crash threshold is not measured between 100 MB and 1 GB; no real network; ADR-008 remains Proposed for the same reasons.

## Round II addendum (2026-10-15) — estimator v3, calibration isolation, and why it stays Proposed

**Estimator v3.** Cardinality (rows matching window and filter) is separated from output rows (after aggregate or LIMIT). A 128×128 spatial histogram of bounding-box centres plus per-column statistics (exact category frequencies, a 65-point quantile sketch for numbers) is built in the background at gateway start and cached in `.gf-cache/`; filter selectivity uses the independence assumption; group counts come from exact frequencies, else from the *same column's* history, else a prior (borrowing another column's count was a bug, fixed). LIMIT is an output cap, never a cardinality: `limit_certain` only when the cardinality lower bound is at least the limit.

**Calibration isolation.** Corrections are looked up by (source, selectivity bucket) with at least 2 runs; else by (layout class, bucket) with at least 3 runs, where the class is `coherence|skew|covering|histogram|filter`; else none (factor 1, wide interval, quality Poor). Interval half-width = the P90 residual of the same context. `kind=count` observations are excluded from selectivity learning.

**Held-out v5** (fresh seed 20261018, never used to fit; run once; frozen calibration `calibration-v3.jsonl`, `--no-learn`; Chrome CDP throttling for LOCAL, FAST_LAN, GOOD_BROADBAND, MOBILE, HIGH_LATENCY):

| measure | result | target |
|---|---|---|
| cardinality error median / P90 / P95 / max | 1.01× / 1.13× / 1.33× / 2.0× | median < 2×, P90 < 5× |
| underestimates by more than 5× | 0 % | — |
| true value inside the interval | 100 % | — |
| planner accuracy / P90 regret | 97 % / 1.00 | always-server: 90 % |
| time model, server: median / P90 | 1.11× / 1.50× | median ≤ 1.25×, P90 ≤ 3× |
| time model, browser: median / P90 | **1.37×** / 2.68× | median ≤ 1.25×, P90 ≤ 3× |

**Not met:** the browser time model's median (1.37×, target 1.25×), and per profile LOCAL (median 1.71×, P90 3.5×) and HIGH_LATENCY (P90 3.4×). The 3 wrong choices (all LOCAL) were near misses (regret 1.04–1.28). One machine, Chromium only, no second device. Hence **Proposed**, not Accepted.

**Invalid earlier runs, kept as evidence.** (1) Held-out v3 used the gateway's network simulation, whose per-request artifact contaminated the browser constants: archived in `ho-v3-cdp-artifact/`. (2) Held-out v4 was served by a *stale gateway left from an earlier run* (all five new gateways failed to bind port 8810 and the harness did not notice), so every case was planned with the MOBILE profile: archived in `ho-v4-invalid-stale-gateway/`, its numbers are not used. The harness now refuses a busy port and checks that the gateway reports the expected network profile; v5 was generated after the fix with a new seed.

## Round III addendum (2026-10-19) — component browser cost model, fresh held-out, calibrated range

**What changed.** The browser time is now a *sum of parts the engine measures itself* (`crates/geo-planner/src/browser_model.rs`): plan round trip, engine start (cold: engine CPU + the 36.7 MB engine assets over the link + first-query warm-up), opening the sources (whole-file mode: transfer + registration), range requests (sequential, each `0.97·RTT + 25.7 ms` on a throttled link + 2.8 ms CPU) and their bytes at link speed, and result materialisation (rows, geometry bytes, attribute bytes). Request counts and bytes of range mode come from planner-known features (row groups kept, covering bytes, LIMIT-bound or not). The estimator gained an **exact row index for sources of up to 100,000 rows** (bbox + WKB length per row, histogram sidecar v3): window cardinality and result geometry bytes are exact for small polygon layers whose objects are larger than the window (administrative boundaries were off 5× in rows and 16× in geometry cost before). Constants were fitted on the calibration set only (159 measured runs, `benchmarks/results/bcal-*.json`, `fit_browser_model.py`), rounded, frozen, and implemented in Rust; Rust and Python agree on the calibration set (median 1.07×/1.07×, P90 1.51×/1.49×).

**Harness artifact found and removed.** Earlier browser timings, including the browser column of held-out v5, were inflated because the harness returned the whole result through Playwright's `evaluate` (serialisation counted as engine time). The harness now returns a summary only. The v5 browser-time numbers (median 1.37×) are therefore not comparable and not used.

**Fresh held-out** (seed 20261019, new data draw `_s5` of the generator, 164 cases, 5 network profiles by Chrome CDP throttling, warm/cold, run once, frozen model and calibration, `--no-learn`, gateway port checked free and profile verified; the four Overture pack layers are the same files as in calibration — only the windows are new):

| measure | result | gate |
|---|---|---|
| browser time model, median | **1.19×** | ≤ 1.25× |
| browser time model, P90 / P95 / max | **2.28×** / 2.54× / 10.2× | P90 ≤ 3× |
| by profile (median / P90) | LOCAL 1.74/2.62, FAST_LAN 1.79/2.44, GOOD_BROADBAND 1.15/1.68, MOBILE 1.04/1.34, HIGH_LATENCY 1.01/1.28 | — |
| whole-file / range mode (median / P90) | 1.15 / 2.30 and 1.41 / 2.06 | — |
| cardinality (median / P90 / max) | 1.00× / 1.19× / 3.14× | median < 2×, P90 < 5× |
| planner accuracy | 96.3 % (always-server 82.3 %) | — |
| regret P50 / P90 / P95 / max | 1.00 / 1.00 / 1.00 / 4.63 | P90 ≤ 1.1 |

**Not clean.** (1) LOCAL and FAST_LAN are under-predicted by 1.7–1.8× on the fresh draw (median measured/predicted 1.74 and 1.79; 1.0–1.13 elsewhere); absolute errors are tens of milliseconds, but it is a bias, not noise. The plan round trip of the same operation was 57 % slower in the held-out run than in calibration (11 vs 7 ms), so some of it may be machine state; this is not proven and the model was not refitted on held-out data. (2) The worst regret (4.6×) is a 25-row administrative-boundary query where the *server* took 2.3 s; the server time model is not covered by this work (median 1.46×). (3) The 8 digest mismatches in the held-out run were all `count_by` cut at 50 groups with ties on the count: the two engines ordered ties differently. Fixed by a deterministic tie-break on the group key (both templates); the 8 cases re-run: 8/8 identical digests (`btie-*.json`; a re-run of the failing cases, not a new held-out).

**Calibrated range (post-hoc, separate evidence).** After the held-out, files outside the calibrated range were measured on purpose: the 1.45 GB buildings file (24 runs) and two 1 GB synthetic files (24 runs), LOCAL. Result: the browser is 1.5–3× slower than the server there (warm), up to 10× cold, while the model predicted it would win on a small window (e.g. 69 ms predicted, 190–320 ms measured). Policy **R5c** therefore keeps the browser out of *automatic* selection for any source larger than the largest file the model was calibrated on (128 MiB); an explicit engine choice still works, and the confidence factor `dataset_familiarity` is halved for such sources. This is a safety rule decided from evidence gathered after the held-out; the held-out files are all ≤ 100 MB so R5c does not change any held-out decision.

**Independent device/browser evidence: insufficient for Accepted.** The in-page device matrix (8 cases, `?devicecheck=1`) ran in Chrome, Edge and Playwright WebKit (a different engine, Windows build) — all on the **same machine**; every runnable case matched the server's result digest and the unsafe candidate stayed on the server. No second physical machine was available. Firefox was not installed (needs a ~90 MB download, not done). Per the acceptance criteria written for this phase, same-machine Edge does not count, and WebKit is partial evidence.

**Status: remains Proposed.** Candidate for Accepted once (a) the device matrix is run on a second physical machine and (b) the LOCAL/FAST_LAN bias is explained or removed. ADR-008 stays Proposed.

## Final validation addendum (2026-10-19, report `REPORT-GEOFABRIC-FINAL-PLANNER-VALIDATION.md`) — status unchanged: Proposed

**Timing contract.** Browser time = ONE clock inside the page, from the moment the placed request leaves to the moment the result exists in the tab: plan request + response (gateway planning included) + engine start (cold only) + open + first batch + drain + conversion. Server time = the same POST through the page. Before this phase the harness clock ran in the test driver and included ~3 driver round trips (and the model's `plan_ms` had absorbed them); the model was therefore recalibrated under the new contract (159 runs, `ccal-*`), refitted, frozen (commit `e909764`), then validated once on a fresh held-out.

**Server model.** The server result term ignored geometry (every row cost the same), so a 25-row administrative-boundary query was predicted at 84 ms and took 818 ms. It now has fitted terms per MB of geometry (155 ms) and attributes (234 ms) of the result, and result bytes on the wire include the GeoJSON text. Server error on the calibration set: median 1.12×, P90 1.71×.

**Fresh held-out v2** (seed 20261026, new data draw `_s6`, 200 cases: 118 synthetic ≤ 100 MB, 37 real pack layers, 45 sources above 128 MiB — 300/500 MB and 1 GB synthetic plus the 1.45 GB Overture file; 5 CDP profiles; frozen model, policy and estimator; run once):

| measure | result | gate |
|---|---|---|
| browser time model, median / P90 / P95 / max (187 measurable cases) | **1.23× / 2.66× / 3.67× / 6.6×** | ≤ 1.25× / ≤ 3× |
| same, sources ≤ 128 MiB only (155) | 1.16× / 2.09× / 2.58× / 3.8× | |
| same, sources > 128 MiB (32) | 1.74× / 5.5× / 6.1× / 6.6× — outside the calibrated range, covered by R5c, no claim | |
| server model, median / P90 / max | 1.40× / 2.0× / 3.7× | |
| planner accuracy | 95.0 % (always-server 81.5 %) | |
| regret P50 / P90 / P95 / max | 1.00 / 1.00 / 1.001 / 1.59 (max absolute penalty 75 ms) | P90 ≤ 1.25 |
| unsafe browser choices / browser crashes / digest or row mismatches | 0 / 0 / 0 | 0 |

**R5c** (no automatic browser choice above 128 MiB; the model's prediction is kept as is and the policy overrides it, recorded in `assessment.policy`): of 45 large-source cases the cost model alone would have chosen the browser 8 times; R5c fired 8 times; the browser was measured faster in 3 of those (by 1.05×, 1.44×, 1.59×; at most 75 ms saved) and slower in 5 (by 1.07× to 1.63×). The model's own error on these cases was 4–6× (predicted 20–80 ms, measured 128–430 ms). No browser selection above 128 MiB occurred. 13 large cases could not be run in the browser at all (fetch estimate above the 220 MB tab threshold): the server is the safe winner. R5c is accepted as designed.

**Not met → status stays Proposed.** (1) No second physical machine was available: the matrix ran in Chrome, Edge, Firefox and WebKit on this one machine (correctness: digests equal in 7/7 runnable cases in each). (2) LOCAL and FAST_LAN remain biased low (median 1.78× and 1.41×; the same bias, 1.74×, appeared on the previous fresh held-out). Partly explained: the server model is under-predicted by the same factor on LOCAL (1.63×), so the shift is common-mode and the relative decision is preserved (every wrong choice cost ≤ 75 ms); range mode has no decode-per-MB term and the whole-file open cost per MB is ~20 % low at 30 MB; a reference probe per batch (fixed query, both engines) varies 2× on LOCAL within one run and ~1 % on MOBILE/HIGH_LATENCY, but does not predict the browser error (correlation 0.16), so host drift alone is not the explanation. The model was not modified after seeing the held-out.


## Acceptance (2026-10-20) — decision of the Principal Architect, with its limits

The second-physical-device validation required by the final-validation addendum was **deferred to P1** by decision, not satisfied by evidence. The gates met on the fresh held-out (browser model median 1.23× / P90 2.66× on sources ≤ 128 MiB: 1.16× / 2.09×; planner accuracy 95 %; regret P90 1.00, max 1.59 = 75 ms; 0 unsafe choices) stand as written above. Binding scope limits of this acceptance:

1. **Timing evidence comes from ONE physical machine** (Chromium-family browsers; Firefox and WebKit only for result correctness). The model carries a measured common-mode bias on fast links (LOCAL/FAST_LAN under-predicted ~1.4–1.8×).
2. **Network profiles are emulated** (Chrome-level throttling), never measured on a real network.
3. Browser auto-execution is validated up to **128 MiB**; larger sources default to the server (R5c).
4. **Second physical-device validation is deferred to P1** (`docs/claude/second-device-check.md`); an unexpected result there reopens this ADR.
5. New engines enter as `Unmeasured` (policy R5d, ADR-013) and are never preferred on predicted cost over a measured engine until they have their own calibration and held-out.
