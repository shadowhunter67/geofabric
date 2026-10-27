# REPORT — Metric semi-join planner safety (2026-10-27)

Policy `metric-semijoin-policy-v1` · cost models `postgis-cost-v3`, `duckdb-semijoin-cost-v3` · risk models `duckdb-timeout-risk-v1`, `postgis-timeout-risk-v1`.
Design record: ADR-015 addendum 2, RFC-003 (R5e), `docs/postgis.md`.

## 1. Executive summary

**Outcome: partial success, as the brief defines it.** The failure mode "PostGIS unvalidated ⇒ DuckDB assumed safe" is gone: the planner now carries a separate timeout-risk verdict per engine (`low | elevated | timeout_risk | unknown`), avoids an engine rated `timeout_risk` when another is not, and says so (`Assessment.uncertainty`) when it cannot. No metric class was promoted to auto-select, and the global P90 regret gate (≤ 1.5) is **not met**.

| | Result |
|---|---|
| Heavy-regime confirmation (70 fresh cases): DuckDB timeouts | 16; all 16 rated `timeout_risk` beforehand and routed to PostGIS. One borderline miss: a plan at P = 0.14 (threshold 0.20) where DuckDB finished at 61.0 s |
| Previous policy on the same set | 18 timeouts, 41 plans on a `timeout_risk` engine |
| Balanced confirmation (320 fresh cases) | 0 unsafe, 0 chosen-engine timeouts |
| Correctness | 0 DuckDB/PostGIS mismatches in every set; Demo B 27,624 ids = independent oracle |
| Global P90 regret | 8.23 (balanced), 4.42 (heavy), 5.53 (all 390) — gate NOT met |
| Validated scope P90 regret | 1.00 (max 3.01, max penalty 81 ms) |

## 2. Corrected historical record

* The earlier "DuckDB crash/abort" in benchmark runs was the host's `ram-emergency-killer` (exit `0xffffffff`), not an engine bug. `benchmarks/run_cost_set.py` now classifies this as `HOST_RESOURCE_KILL`.
* Earlier PostGIS timings were inflated by a connector bug (`sleep(15ms)` + `join()` in the cancel watcher, ~15 ms per query). Fixed in the previous phase; now a structural regression test (`watch.rs`).
* The two held-out cases of the previous phase (DuckDB timeout at 60 s, PostGIS 12–18 s) are fixed regression cases; both now plan on PostGIS.

## 3. Method and stages

| Stage | Cases | Seed(s) | Purpose |
|---|---|---|---|
| Discovery | 480 (earlier sets re-planned with histograms + 150 new) | 20261201 | features, class thresholds, which regime times out |
| Calibration | 200 | 20261205 | fit cost + risk models |
| **Freeze** | — | — | commit `4a174f0`: features, classes, models, thresholds, gates |
| Confirmation (balanced) | 320 | 20261209 | gates |
| Confirmation (heavy regime) | 70 | 20261213 | the balanced set had only 10 heavy cases; the heavy set was generated before it was run |

Seeds were new (not 20261103/07/11/15/19 or 20261018/19/26). Gates are unchanged (`benchmarks/postgis_cost_gates.json`). Confirmation is 390 cases (≥ 300). Timeouts are censored at 60 s everywhere. The harness builds histograms like the gateway does (`Runtime::ensure_histogram`), so it measures the planner that ships. Planner decisions in the evaluation are the real ones recorded in each run, not a Python replay.

## 4. Plan-time features (null = unknown, never zero)

Left/right rows (right after its attribute filter), expected probes, radius, right density, bbox overlap, `candidates_per_probe = density × π d²`, `candidate_pairs = probes × candidates_per_probe`, window areas. Footer-only estimates were poor (left rows P90 ×5.3, right ×13); with histograms ×1.3–1.8 and ×1.6–4.6.

## 5. Models

* **Classes** — `semijoin_metric_small` (≤ 5,000 probes), `semijoin_metric_medium` (≤ 50,000 probes and < 1e7 pairs), `semijoin_metric_large` (the rest; unknown pairs ⇒ large). All 26 observed timeouts have ≥ 7.8e7 pairs.
* **Cost** — pooled log-linear Tobit per metric class; other classes keep v2 coefficients (not refitted).
* **DuckDB timeout risk** — Tobit, only for ≥ 1e7 pairs (features ln pairs, ln probes). **PostGIS timeout risk** — only for ≥ 50,000 probes. Below the regime ⇒ `low`; outside the measured envelope or missing feature ⇒ `unknown`. Thresholds: elevated ≥ 0.10, timeout_risk ≥ 0.20.
* PostGIS hard risk (> 50,000 probes) kept, as an overridable cost-based block; 250k warning kept as a warning.
* **Uncertainty (R5e)** — see ADR-015 addendum 2. No speculative dual execution; at most one fallback, none after cancel, no same-engine retry (unchanged).
* Python and Rust are pinned to each other by fixtures (`cost_model_parity.rs`: 36 model fixtures, 10 timeout regression cases).

## 6. Results

### Balanced confirmation (320)

Unsafe 0; chosen-engine timeouts 0; P50/P90/P95/max regret 1.0 / 8.23 / 37.6 / 137.6; mean penalty 174 ms, max 9.5 s. Validated scope (n = 149): P90 1.00, max 3.01. Baselines: previous policy P90 8.92 (mean penalty 404 ms, 3 unsafe), always-DuckDB P90 118, always-PostGIS P90 3.24 (benchmark only). The 8 DuckDB timeouts in this set were admin semi joins in the validated class, already routed to PostGIS.

### Heavy-regime confirmation (70)

DuckDB timeouts 16; 1 unsafe selection (`msj_heavy-0025`, DuckDB p = 0.139, finished at 60.98 s). Regret P90/P95/max 4.42 / 5.16 / 5.53; mean penalty 4.25 s; max 49.7 s. Previous policy: P90 13.7, mean penalty 19.7 s, 18 timeouts, 41 unsafe. Always-PostGIS: P90 11.0. Alternate-engine-saved cases: 17.

### All 390 (388 decidable)

P50/P90/P95/max regret 1.0 / 5.53 / 28.4 / 137.6; mean penalty 910 ms; max 49.7 s; unsafe 1; timeouts by chosen engine: DuckDB 1 (alternate completed), PostGIS 0. Previous policy: 44 unsafe, 18 timeouts, mean penalty 3.9 s. Always-DuckDB P90 103; always-PostGIS P90 4.19 (mean penalty 725 ms). Alternate-engine-saved: 17.

### Per class (all 390)

| Class | n | P90 regret | Note |
|---|---|---|---|
| aggregate | 51 | 1.00 | validated |
| indexed_bbox | 71 | 1.00 | validated |
| semijoin_intersects | 27 | 1.00 | validated |
| broad_scan | 18 | 2.36 | limited; penalty ≤ 134 ms |
| indexed_spatial | 21 | 87.98 | limited; ratio from 1–5 ms PostGIS vs DuckDB; penalty ≤ 178 ms |
| semijoin_metric_small | 46 | 28.4 | limited; same sub-second effect |
| semijoin_metric_medium | 41 | 1.29 | limited |
| semijoin_metric_large | 113 | 4.42 | limited; the multi-second penalties |

### Promotion experiments (fresh balanced set, gates as committed)

`semijoin_metric_small`: regret P90 1.00 but prediction P90 ×3.48 / median ×1.70 (limits 3.0 / 1.6) → no. `semijoin_metric_medium`: regret P90 1.77 → no. `semijoin_metric_large`: regret P90 1.61 → no. No new validated class.

## 7. Decision table

| Class | PostGIS validation | PostGIS risk | DuckDB risk | Auto-select | Fresh evidence |
|---|---|---|---|---|---|
| indexed_bbox | validated | low | low | yes | 71 cases, regret P90 1.00 |
| aggregate | validated | low | low | yes | 51, 1.00 |
| semijoin_intersects | validated | low | low | yes | 27, 1.00 |
| indexed_spatial | limited | low | low | no (R5d) | 21, fails regret |
| broad_scan | limited | low | low | no (R5d) | 18, P90 2.36 |
| semijoin_metric_small | limited | low | low | no | prediction gate fails |
| semijoin_metric_medium | limited | elevated at > 50k probes (overridable) | per model (regime ≥ 1e7 pairs) | no; R5e may pick PostGIS on risk only | regret gate fails |
| semijoin_metric_large | limited | block > 50k probes (overridable) | timeout_risk model | no; R5e may pick PostGIS on risk only | regret gate fails |

## 8. Timeouts (60 s, censored)

| Set | DuckDB timeouts | rated timeout_risk in advance | PostGIS timeouts | chosen-engine timeouts |
|---|---|---|---|---|
| Balanced (320) | 8 (admin class) | n/a — validated class routed to PostGIS | 0 | 0 |
| Heavy (70) | 16 | 16 / 16 | 0 | DuckDB 1 (a 61.0 s completion), PostGIS 0 |
| Fixed regression (10 earlier) | 10 | 10 / 10 (CI test) | — | — |

False alarms: 7 heavy-set plans and 3 balanced-set plans rated `timeout_risk` where DuckDB would have finished; PostGIS was 4–15× faster in them.

## 9. Infrastructure

* `HOST_RESOURCE_KILL` classification; structural test for the old PostGIS latency tax; 10 ms ratio floor documented (gates amendment 2026-10-26).
* E2E: gateway build identity recorded (`/api/health` ← `build.rs`); e2e fails on mismatch with HEAD; `GEOFABRIC_POSTGIS_*` blanked unless `GF_E2E_POSTGIS=1`.
* Developer panel shows class, validation, predicted cost, risk, candidate-pair estimate and decision; Explorer unchanged.
* Planner logs carry model versions, features, risk states and decision reasons.

## 10. Verification

* Full-stack Playwright with a fresh gateway (identity = `bc9a6ae`, not dirty): **Chrome 61 passed, Edge 61 passed**, 4 skipped by design, 0 failed.
* Live cross-engine: 17/17 plans identical, Demo B 27,624 ids = oracle, 8 live PostGIS tests passed.
* Static-demo and CI results: see the final response.

## 11. Limitations and risks

* The global P90 regret gate is unmet; it is dominated by limited classes with sub-second absolute penalties and by metric semi joins where the safe choice costs time (false alarms).
* The risk models were fitted on one machine, one dataset family (HCMC), one PostgreSQL build; thresholds can drift on other hardware. Second machine / remote PostgreSQL remains open (P1).
* One heavy-regime DuckDB timeout-adjacent case (61 s) fell just under the 0.20 threshold: the tail is not zero.
* DuckDB heavy-regime expected time is only good to ~P90 ×9; safety rests on the risk verdict, not on the point estimate.
* PostGIS physical design (per-class indexes) is unchanged and could move several classes.

## 12. Remaining P0/P1

* P0: none open.
* P1: second machine / remote PostgreSQL; revisit `semijoin_metric_small` prediction model (regret already 1.00); evaluate (on new data, not this confirmation set) whether a lower risk threshold would close the 61 s miss.

## 13. Recommended next phase

One fresh held-out round on a second machine for `semijoin_metric_small` and `semijoin_metric_medium` only (≥ 15 fresh cases each, pre-committed gates), then decide promotion. Do not touch the frozen classes.
