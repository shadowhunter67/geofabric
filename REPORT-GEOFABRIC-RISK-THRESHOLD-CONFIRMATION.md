# REPORT — DuckDB timeout-risk threshold confirmation + metric_small research (2026-10-28/29)

Shipped models unchanged: `postgis-cost-v3`, `duckdb-semijoin-cost-v3`, `duckdb-timeout-risk-v1` (timeout_risk at **0.20**), `postgis-timeout-risk-v1`; policy `metric-semijoin-policy-v1`. No planner, model or UI code is changed in this phase, so no version was bumped. A tested alternative (`duckdb-timeout-risk-v2` at 0.10, policy v1.1) is saved as a patch, not applied (§9).

## 1. Executive summary

**Decision D — by the pre-committed rule no frozen candidate is sufficiently safe; the threshold stays at 0.20. Label: RISK MODEL NEEDS FEATURE IMPROVEMENT.** On 140 fresh heavy-regime cases (136 decidable) the frozen thresholds 0.20 / 0.15 / 0.10 give 10 / 5 / **1** unsafe choices. The frozen primary gate is unsafe == 0; none reaches it, so nothing was changed.

0.10 is nevertheless clearly better than 0.20 on every measure: unsafe 10 → 1, chosen-engine timeouts 10 → 1, P90 regret 15.4 → 8.97, mean penalty 18.3 s → 5.4 s, 0 unnecessary reroutes; on the 390 earlier cases unsafe 1 → 0. The one remaining unsafe case (`thr_pool2-0726`, DuckDB P = 0.144, 948k probes) cannot be rerouted by any threshold: PostGIS is also infeasible for a non-overridable reason (estimated result 1.095 M rows > the 1 M row cap), and with 0.10 the planner does attach an uncertainty note. I did not relax the frozen definition after seeing this. The change is prepared and tested (`benchmarks/results/postgis-cost/proposed-threshold-0.10.patch`) for an explicit decision.

Why 0.20 fails: in all 10 unsafe cases PostGIS was blocked only by its overridable >50,000-probe cost block (probes 83k–948k; PostGIS finished each in 2.6–22 s). R5e lifts that block only when DuckDB is `timeout_risk`, so at DuckDB P = 0.10–0.20 the planner stays on DuckDB. The old P = 0.139 / 61 s miss was not a one-off.

`semijoin_metric_small` is **not promoted (outcome C, with an offline candidate)**: candidate pairs are badly estimated but PostGIS time does not depend on them; a 256-grid histogram improves left/right estimates a lot and held-out time prediction modestly (P90 ×2.98 → ×2.63).

**Correction during the phase:** my first replay of thresholds below 0.20 treated PostGIS's overridable block as not liftable and printed identical rows for all three thresholds (a harness bug, not a finding). I found it by reading the planner (`safe_deferred`), fixed `pg_ok` to lift only blocks whose every rejection reason is `risk policy`, and validated the replay against the real planner built with the 0.10 patch: they agree on all 136 decidable cases. The pre-fix table was never used for a decision.

## 2. Previous state

R5d class-scoped validation + R5e (`metric-semijoin-policy-v1`): DuckDB `timeout_risk` at P ≥ 0.20, `elevated` ≥ 0.10. Previous heavy set (70): 16 DuckDB timeouts all rerouted, one miss at P = 0.139 (61.0 s). Global P90 regret gate unmet; metric_small/medium/large limited.

## 3. Frozen threshold candidates

Committed in `a69b081` before any confirmation run (`benchmarks/risk_threshold_candidates.json`): θ ∈ {0.10, 0.15, 0.20} (current 0.20). Unnecessary reroute = DuckDB completes < 60 s and R5e moved to PostGIS and t_PostGIS ≥ 2×t_DuckDB and difference ≥ 1000 ms. Rule: unsafe == 0 → chosen-engine timeouts == 0 → least aggressive threshold with fewest unnecessary reroutes; none passes → keep 0.20 and report. No candidate added afterwards.

## 4. Benchmark environment

Portable PostgreSQL 16.9 + PostGIS 3.6.2 (127.0.0.1:5433), buildings 3.85 M, transport 453,550; same 32 GB machine. Release build, harness `postgis_cost.rs` via `benchmarks/run_cost_set.py`; engine order alternated per case, warm-median repeats, timeouts censored at 60 s. Watchdog (ram-emergency-killer) active before the run; **0 watchdog entries, 0 HOST_RESOURCE_KILL, 0 engine crashes** (1 attempt, 10,476 s). Run on commit a69b081 plus untracked benchmark scripts.

## 5. Fresh heavy confirmation

| | |
|---|---|
| Pools | seeds 20261301 (900) and 20261303 (4,000), plan-only; selection seed 20261302 (`select_thr_set.py`, plan-time features only, never a measured time) |
| Cases | **140** (136 decidable) |
| Coverage | 70 cases in the borderline bands 0.08–0.12 / 0.13–0.17 / 0.18–0.22, plus a probes × candidate-pairs grid filler; radii 100/200/500/1000 m; dense/medium/sparse regions; filtered and unfiltered |
| Outcomes | DuckDB: 105 ok / 32 timeout; PostGIS: 139 ok / 0 timeout |
| Correctness | **0 mismatches** among 105 comparable cases (35 not comparable because DuckDB timed out) |

## 6. Threshold comparison (140 fresh, 136 decidable)

| θ | unsafe | chosen-engine timeouts | reroutes | unnecessary | alternate-engine-saved | P50 | P90 | P95 | mean pen. | max pen. |
|---|---|---|---|---|---|---|---|---|---|---|
| 0.10 | **1** | 1 | 83 | 0 | 32 | 1.00 | 8.97 | 15.2 | 5.39 s | 44.3 s |
| 0.15 | 5 | 5 | 58 | 0 | 28 | 2.04 | 14.2 | 19.8 | 11.96 s | 58.2 s |
| **0.20** (current) | 10 | 10 | 33 | 0 | 23 | 3.57 | 15.4 | 20.1 | 18.30 s | 58.2 s |

The replay equals the real planner at 0.20 on every decidable case of both sets, and equals the real planner built with θ = 0.10 on all 136 fresh decidable cases. Context (390 earlier cases, not used for selection): unsafe 0 / 1 / 1, reroutes 29 / 27 / 26, P90 5.50 / 5.50 / 5.53, mean penalty 664 / 853 / 910 ms. Baseline on the fresh set: always-DuckDB P90 17.2, mean penalty 29.4 s. Real planner with the 0.10 patch: 86 PostGIS / 54 DuckDB, 3 plans with an uncertainty note.

## 7. Unsafe choices and timeouts

Ten unsafe at 0.20 (all DuckDB; PostGIS finished each):

| id | DuckDB P | probes | PostGIS time | at 0.10 |
|---|---|---|---|---|
| thr_pool-0791 | 0.101 | 782,564 | 19.4 s | rerouted |
| thr_pool2-2991 | 0.110 | 83,479 | 3.0 s | rerouted |
| thr_pool2-0253 | 0.142 | 212,263 | 7.2 s | rerouted |
| **thr_pool2-0726** | 0.144 | 948,184 | 22.0 s | **stays DuckDB**: PostGIS over the 1 M-row cap too; uncertainty note |
| thr_pool2-3190 | 0.148 | 435,688 | 17.7 s | rerouted |
| thr_pool2-0427 | 0.154 | 305,492 | 4.8 s | rerouted |
| thr_pool2-0665 | 0.162 | 148,774 | 3.1 s | rerouted |
| thr_pool-0797 | 0.166 | 613,741 | 12.1 s | rerouted |
| thr_pool-0703 | 0.189 | 236,181 | 10.9 s | rerouted |
| thr_pool2-0743 | 0.199 | 105,293 | 2.6 s | rerouted |

33 DuckDB runs reached 60 s (32 status timeouts, the rest completions at 60.1–61.2 s): P ∈ [0.101, 0.996]; 13 at P < 0.22, the rest ≥ 0.46. PostGIS timeouts: 0. At 0.20 these ten carry no uncertainty note.

## 8. Reroute analysis

Reroutes 33 / 58 / 83 at θ = 0.20 / 0.15 / 0.10; unnecessary reroutes **0 at every θ**. Cases with P ≥ 0.10 and PostGIS blocked only by the overridable block: 52 in the fresh set, 0 PostGIS timeouts among them.

## 9. Selected risk threshold

**D — keep 0.20** (frozen rule: primary gate unsafe == 0 on the fresh heavy set; otherwise keep 0.20 and report RISK MODEL NEEDS FEATURE IMPROVEMENT).

For the decision-maker: 0.10 is the least bad candidate by a wide margin, fails only the primary gate and only by one case no threshold can influence, and has no measurable cost. Applying it means either accepting "unsafe ≤ 1, with an uncertainty note" instead of "unsafe == 0", or first fixing the row-cap interaction. The prepared change: `duckdb-timeout-risk-v2` (`timeout_risk_at` 0.10, `elevated_at` 0.05, coefficients unchanged), `metric-semijoin-policy-v1.1`, regenerated model fixtures and a fixed regression test for the eleven missed cases (`timeout-regression-cases-r2.json`, kept in the repo as evidence). With the patch applied the planner/executor/runtime tests passed. Since 0.10 would be chosen from this set, a fresh seed would be its proper confirmation.

## 10. Metric_small research

Seeds: discovery 20261401 (151 usable cases), calibration 20261405 (133), confirmation 20261409 (76, prepared, **not run**: no shipped feature to confirm). Truth from PostgreSQL (`metric_small_truth.py`): left, right, and pairs = (building, road) with road bbox ∩ building bbox grown by the distance.

## 11. New feature experiments

PostGIS time model fitted on discovery, held out on calibration (ratio with 10 ms floor):

| model | held-out median | held-out P90 |
|---|---|---|
| current [ln probes, ln right] | ×1.55 | ×2.86 |
| + ln candidate_pairs (est) | ×1.56 | ×2.98 |
| + ln distance | ×1.52 | ×2.77 |
| probes, right, pairs, d | ×1.54 | ×2.95 |
| ORACLE true left, right | ×1.50 | ×2.38 |
| ORACLE true pairs only | ×2.04 | ×4.24 |
| ORACLE left, right, pairs, d | ×1.47 | ×2.44 |

Candidate pairs add nothing even when exact (coefficient ≈ 0.0–0.04): PostGIS metric_small time is driven by probes. Even perfect inputs only reach P90 ≈ ×2.4; the rest is noise at 10–50 ms.

Histogram granularity (centre-count grid, fractional overlap; truth from PostgreSQL):

| grid | left med/P90 | right med/P90 | held-out time pred. med/P90 | size (full extent, int32) |
|---|---|---|---|---|
| 128 (shipped grid) | ×1.27 / ×2.62 | ×1.22 / ×4.00 | ×1.54 / ×2.98 | 0.1 MB |
| 256 | ×1.14 / ×1.51 | ×1.13 / ×1.99 | ×1.47 / ×2.63 | 0.3 MB |
| 512 | ×1.09 / ×1.39 | ×1.12 / ×1.80 | ×1.51 / ×2.55 | 1.0 MB |
| 1024 | ×1.04 / ×1.19 | ×1.10 / ×1.77 | ×1.48 / ×2.45 | 4.2 MB |

The shipped planner's own estimate is left ×1.26/×2.65 and right ×1.44/×6.0 (its right path is worse than the pure grid). Build time and planner latency of a finer grid were not measured: no Rust change was made. The grid numbers are from `metric_small_grid_eval.py`; sizes are the dense full-extent upper bound.

## 12. Estimation error decomposition

Planner estimates vs truth (284 usable small cases): left rows median ×1.24–1.28, P90 ×2.5–2.7 (bias 1.03–1.05); right rows after the class filter median ×1.3–1.5, P90 ×4.3–7.6 (bias 0.85–0.9); **candidate pairs median ×4.5, P90 ×74–126, bias 0.33–0.34** (about 3× under, because the window-uniform density ignores road-bbox size and clustering). PostGIS time error is dominated by the floor/noise, not by these estimates. The DuckDB time model was not refit.

## 13. Metric_small confirmation

Not run: nothing new to confirm. Promotion gates (≥ 15 fresh cases, regret P90 ≤ 1.5, 0 unsafe, prediction median ≤ 1.6 / P90 ≤ 3.0 at the 10 ms floor) are unchanged. **Outcome C (limited): no feature improves the shipped model; a 256+ grid is a documented candidate.** `cases-ms_conf.json` (seed 20261409) stays frozen for it.

## 14. Class decision table

Unchanged from `REPORT-GEOFABRIC-METRIC-SEMIJOIN-SAFETY.md` §7: indexed_bbox / aggregate / semijoin_intersects validated; indexed_spatial, broad_scan, metric_small/medium/large limited (R5d). metric_medium/large not tuned.

## 15. Demo B, wide metric case, cross-engine correctness

Not re-run: no engine, planner, lowering or gateway code is changed in the repo since 5a26034, where Demo B = 27,624 ids = independent oracle and the wide case = 189,772 ids on both engines were verified. Fresh evidence this phase: 0 DuckDB/PostGIS mismatches in 105 comparable heavy cases and in the 284 usable metric_small cases. Live: **8/8 PostGIS tests passed**; planner/executor/engine/runtime tests green (also with the 0.10 patch applied).

## 16. Full-stack regression

Chrome/Edge full-stack and the static suite were **not re-run**: only benchmark scripts and evidence changed; the shipped gateway and web bundle are the ones verified in the previous phase (Chrome 61 + Edge 61 passed at `bc9a6ae`). Carried over, not re-measured.

## 17. CI / Pages

See the final response (push result). `tools/repo_guard.py` OK and `tools/arch_check.py` OK locally; raw traces and pool files stay git-ignored/untracked.

## 18. Architecture risks

* R5e can only act where the alternative is liftable; non-overridable limits (the 1 M-row result cap) make timeout avoidance impossible in a few heavy cases.
* PostGIS's >50k-probe block is a cost judgement but acts as a safety veto while DuckDB P < 0.20.
* Candidate pairs is a poor absolute estimate (×0.33 bias); it works as a regime flag only because the risk model absorbed the bias, and should not be a cost driver.
* The 128-grid histogram leaves left/right errors that a 256 grid halves.
* A replay harness can silently disagree with the planner (it did once); always check replay == real planner.

## 19. Remaining P0

None for correctness (0 DuckDB/PostGIS mismatches). Open decision: apply the 0.10 patch or not (§9); until then 10 of 136 heavy-regime plans can pick a DuckDB that times out while PostGIS would finish.

## 20. Remaining P1

1. Decide on 0.10 (or on a soft row cap for R5e) and confirm it on a new fresh seed (≥ 120 heavy cases); keep `timeout-regression-cases-r2.json` as the fixed regression set.
2. 256-grid histogram candidate for metric_small (and the right-row estimate): versioned model, frozen confirmation set `cases-ms_conf.json` (seed 20261409, 76 cases).
3. Second machine / remote PostgreSQL.

## 21. Recommended next phase

"Threshold 0.10 decision + fresh confirmation": apply the saved patch only after the decision, then run a new-seed heavy set (≥ 120, borderline bands again) with the real v2 planner; in the same phase address the row-cap interaction for R5e. Ship the 256-grid histogram only together with a metric_small refit and the `ms_conf` confirmation.
