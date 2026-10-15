# ADR-010 — Planner confidence, engine health, failure taxonomy, network model

Status: Proposed (2026-10-14)

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
