# REPORT-GEOFABRIC-RESULT-RISK-POLICY

Phase: result-cap / risk interaction, bounded result policy, fresh confirmation of the 0.10 DuckDB timeout threshold. 2026-10-30. Branch `main`. Freeze commit `c35ba19`.

## 1. Executive Summary

* The 1M-row cap is a result-DELIVERY limit. It was applied by the planner as a PostGIS-only infeasibility, which hid it from R5e and could leave a timeout-risk DuckDB as the "only" choice. It is now an engine-neutral, versioned policy (`result-policy-v1`, ADR-016) that judges the plan's OUTPUT only and answers `safe | bounded | uncertain | too_large`; `too_large` is `NO_SAFE_PLAN` with valid alternatives. Aggregates over more than 1M matches stay legal; raw >1M stays blocked; nothing is ever silently cut to "first N".
* Fresh confirmation of 0.10 (156 cases, seed 20261501, definitions and candidate frozen in `c35ba19` BEFORE the run): **gate missed** (3 avoidable unsafe choices, gate 0). **Final decision C: shipped threshold stays 0.20**, the candidate was reverted, no new threshold search. 0.10 is clearly better than 0.20 on this set (timeouts 8 -> 3, regret P90 7.0 -> 4.2, mean penalty 13.6 -> 5.0 s) but the residual misses are model blind spots, not a constant.
* New findings: (a) result-policy-v1 is conservative: 22 of its 27 refusals would have fit (it judges the point estimate, which is biased up); (b) DuckDB runs out of memory on large results (38 of 156 DuckDB runs) - a materialization-risk class no model covers.
* metric_small 256-cell histogram: estimates and held-out PostGIS time prediction improve; status **limited, improved, not promoted**.
* 0 correctness mismatches, 0 result-cap violations. Full-stack regression green. 295 Rust tests pass.

## 2. Previous Decision

Previous phase (63e64f0): decision D, keep 0.20; 0.10 patch saved as `proposed-threshold-0.10.patch`; risk model "needs feature / policy improvement"; metric_small/medium/large limited; R5d and R5e keep. Approved by the Principal Architect with the 0.10 patch as a strong candidate, not shipped.

## 3. Current Result Cap

| where | value | role |
|---|---|---|
| `geo_executor::limits::DEFAULT_MAX_ROWS` | 1,000,000 | runtime default for `ExecutionContext.max_rows` |
| gateway | `MAX_FEATURES + 1` = 100,001 | what the public gateway uses |
| PostGIS descriptor `max_result_rows` | `Some(DEFAULT_MAX_ROWS)` | declared engine limit (before: made PostGIS infeasible in the planner) |
| DuckDB descriptor | `EngineLimits::default()` (none) | no declared cap |
| browser | 500k (planner `browser_max_result_rows` 100k) | browser materialization |
| enforcement | after execution in every engine (`LimitExceeded`) and `LIMIT row_cap(max_rows)` in the generated SQL | runtime backstop |

## 4. What the 1M Cap Protects

Memory and wire cost of MATERIALIZING and handing on a result (Arrow batches, JSON/GeoJSON, browser), not the cost of scanning or matching. It does not limit input rows, and it cannot protect against an engine timeout. Evidence: `COUNT(*)` over 2 M matches returns one row and passes; `near_ids` returning 1.2 M rows trips `LimitExceeded` on any engine.

## 5. Execution Risk vs Result Risk

| | question | owner | outcome |
|---|---|---|---|
| execution risk | will this engine finish before the timeout | engine cost model + R5e | `low/elevated/timeout_risk/unknown/high`, may be overridable |
| result-delivery risk | can the FULL output be materialised and delivered | `result_policy` | `safe/bounded/uncertain/too_large`, same for every server engine |
| transport / render | is the payload sensible for the client | render sampling (ADR-012) | never changes analytics |

Delivery is not an engine property and no longer makes an engine "infeasible" for reasons only that engine declares.

## 6. Result Policy Design

`geo_planner::result_policy::assess(rows, rows_high, bytes, cap, bounded_by_limit)`. `cap = min(ExecutionContext.max_rows, server engines' max_result_rows)`.
`rows > cap` -> `too_large`; `bounded_by_limit && rows_high <= cap` -> `bounded`; `rows_high > cap` -> `uncertain`; else `safe`. Output rows only (after aggregate and an explicit certain LIMIT). Bytes and geometry weight are carried in the verdict. `Assessment.result_delivery` carries version, verdict, rows, rows_high, bytes, cap, reason. The policy never rewrites the plan. Policy versions: `result-policy-v1`, `metric-semijoin-policy-v1.1` (R5e plus delivery uncertainty notes). Streaming/pagination: P1, not done.

## 7. R5e Interaction

`too_large` removes every server engine (the same message for all), so there is no "least bad" pick: `NO_SAFE_PLAN (the full result cannot be delivered): ...`. `uncertain` on the chosen server engine is added to `Assessment.uncertainty` next to any timeout note. A delivery block is not an execution risk and never makes a timeout-risk engine look safe or unsafe. An explicit engine hint still bypasses R5e but not the runtime cap.

## 8. Known Remaining Unsafe Case

`thr_pool2-0726` (near_ids, raw features, estimate 1,095,005 rows, pessimistic 2,371,958): before, DuckDB (timed out at 60 s) was chosen because PostGIS was "infeasible" by the cap. Now: `NO_SAFE_PLAN` with alternatives; fixed regression `result_policy::tests::thr_pool2_0726_is_a_labelled_refusal_with_valid_alternatives`. Honest note: PostGIS actually returned 925,879 rows (under the cap) - v1 refuses on the estimate (see 22).

## 9. Bounded Result Semantics

An explicit, certain user `Limit` that is within the cap makes the result `bounded`. A limit that is not certain (`limit_certain` false) or above the cap bounds nothing. A result is never shortened by the system: it is delivered whole or refused. The only truncation is the user's own LIMIT. Render sampling thins what is DRAWN; the analytical result, counts and the exported ids are unchanged.

## 10. Aggregate / Limit / Raw Result Cases

Unit tests (all pass): aggregate over a huge input is `safe`; explicit LIMIT under the cap is `bounded`; LIMIT above the cap is `too_large`; raw above the cap is refused for every server engine; the boundary is exact (cap rows safe, cap+1 refused, cap with pessimistic cap+1 uncertain).

## 11. Policy Regression Tests

`result_policy` (7 unit tests) and `uncertainty_policy` (12): aggregate >1M allowed; explicit LIMIT allowed and bounded; raw >1M refused for every server engine; timeout-risk DuckDB next to a delivery-blocked PostGIS is `NO_SAFE_PLAN` not a false-safe pick; delivery blocking is not an engine execution risk; `max_rows_limit_enforced` accepts the earlier plan-time refusal. Render sampling independence is covered by the existing `sampling.spec.ts` (green on Chrome and Edge).

## 12. 0.10 Candidate Freeze

`benchmarks/result_policy_freeze.json` + commit `c35ba19`, before the run: candidate `duckdb-timeout-risk-v2` (`timeout_risk_at 0.10`, `elevated_at 0.05`, coefficients unchanged), `metric-semijoin-policy-v1.1`, `result-policy-v1`; four outcome classes (`NO_SAFE_PLAN`, `UNAVOIDABLE_UNCERTAIN`, `AVOIDABLE_UNSAFE`, `RESULT_CAP_VIOLATION`); gate 0/0/0/0; no new threshold search; excluded seeds 20261301/02/03, 20261401/05/09. Selection predictions frozen in `rp_heavy-frozen-predictions.json`.

## 13. Fresh Heavy Confirmation

Pool seed 20261501 (6000 candidates), selection seed 20261502, plan-time only stratification, **156 cases** (>= 150). Bands: P(timeout) <0.08: 25, 0.08-0.12: 21, 0.12-0.18: 24, 0.18-0.22: 21, >=0.22: 21, refused (n/a): 27 and 17 without a DuckDB estimate. Shapes raw_geom / raw_ids / limit / count / near_limit. Estimated-row strata around 500k / 800k / 950k / 1.0M / 1.05M / 1.2M / 2M+. One attempt, 12,928 s, no kill, no crash, watchdog active with no lines (`HOST_RESOURCE_KILL`: none).

## 14. Unsafe Choices

Real planner (0.10): `SAFE` 118, `NO_SAFE_PLAN` 27, `UNAVOIDABLE_UNCERTAIN` 8, **`AVOIDABLE_UNSAFE` 3**.

| id | shape | P (DuckDB) | level | note attached | DuckDB | PostGIS |
|---|---|---|---|---|---|---|
| rp_pool-1086 | near_count | 0.00 | low | no | timeout | 34.8 s |
| rp_pool-5104 | near_ids | 0.0998 | elevated | yes | timeout | 14.2 s |
| rp_pool-5990 | near_ids | 0.082 | elevated | yes | timeout | 11.4 s |

One is a model blind spot (count over a large right side, P = 0), two sit just under the 0.10 line. A lower constant would change one or two and break the frozen no-search rule; the honest label is "risk model needs feature improvement".

## 15. Timeout Analysis

Chosen-engine timeouts (real planner): 3, all with a liftable alternative that completed. Replay at 0.20 (parity with the real planner verified on all 129 planned cases): 8. DuckDB timeouts across all 156 runs: 31; PostGIS timeouts: 5. Alternate-engine-saved (PostGIS chosen, DuckDB timed out, PostGIS finished): 22.

## 16. No-Safe-Plan Cases

27 refusals, all with message `NO_SAFE_PLAN (the full result cannot be delivered)`. Ground truth: 5 truly unrunnable or above the cap (both engines `limit_exceeded`/timeout, 0 rows returned), **22 would have fit** (actual 144k-937k rows; the estimate was above 1M, estimate / actual median x1.42, max x19.8). On the 106 planned feature cases the same estimator is x1.17 median, x1.52 P90, x19.3 max, and the pessimistic bound covered the actual in 106/106. The policy is safe but too conservative at the estimate boundary. Not tuned in this phase (it would be tuning on the confirmation set). P1: `result-policy-v2`: refuse only when the optimistic bound is above the cap, treat the band between as `uncertain` and let the runtime cap decide.

## 17. Regret and Penalty

Real planner at 0.10, 83 decidable planned cases: regret P50 1.0, P90 4.24, P95 6.24, max 20.63; mean penalty 5.0 s, max 48.6 s; optimal share 78%. Replay at 0.20: P90 7.02, P95 11.17, max 20.63; mean 13.6 s, max 56.6 s; optimal 53%. The global P90 <= 1.5 gate (from earlier phases) is still not met.

## 18. Metric Small 256-Grid Experiment

Set `ms_conf` (seed 20261409, 76 cases, 0 mismatches, 148 s), truth from PostgreSQL, time model [ln_probes, ln_right] fitted only on ms_disc + ms_cal.

| | planner 128 | grid 256 |
|---|---|---|
| left count error median / P90 / max | x1.29 / x2.81 / x7.3 | x1.16 / x1.61 / x2.9 |
| right count error median / P90 / max | x1.56 / x5.32 / x23.6 | x1.14 / x1.86 / x4.0 |
| held-out PostGIS time prediction median / P90 / max | x1.41 / x3.18 / x8.05 | x1.32 / x2.53 / x7.05 |
| `candidate_pairs` error P90 | x32.5 (unchanged, not a grid quantity) | - |
| metadata (int32 cells) | 95 KB | 364 KB (x3.8) |
| grid build (PostgreSQL group-by) | ~1.5 s | ~1.4 s |
| estimator cost (numpy mirror, per buildings estimate) | 124 us | 188 us |

Status: limited, **improved**; prediction meets the earlier 3.0 / 1.6 gate on this set, but there is no Rust 256 grid, no regret test through the planner and `candidate_pairs` is unchanged, so no promotion. Latency numbers are a numpy proxy, not planner end-to-end.

## 19. Correctness Regression

0 DuckDB/PostGIS mismatches in 73 comparable heavy cases and 0 in 76 metric_small cases. Live cross-engine: 17/17 plans IDENTICAL on the slice window; Demo B 27,624 ids identical (`demo_b_ids_template_shape`); wide window: 189,772 ids identical on both engines (DuckDB 1.9 s, PostGIS 16.6 s). On the wide window `semi_join_intersects_admin` hit the 240 s test timeout on DuckDB (57-67 s already on the slice window): not an identity failure, not a regression from this phase (engines unchanged), but it means that case has no wide-window equality evidence in this phase.

## 20. Full-Stack Regression

8/8 live PostGIS driver tests. Playwright with a freshly built release gateway (identity check refused a stale `bc9a6ae8e0` stamp, rebuilt, `c35ba19246 dirty` OK): **Chrome 61/61, Edge 61/61** (a first Chrome pass had one failure in `planner.spec.ts:130` "fallback is bounded"; it passed alone and in both later full runs: flaky, not caused by this phase). Static profile smoke built locally (`vite --mode static --base /geofabric/`): Chrome 13/13, Edge 13/13. `arch_check` OK, `repo_guard` OK. `cargo test --workspace --no-default-features`: 295 passed, 0 failed. UI: developer mode shows `result_delivery`; Explorer shows one sentence for `NO_SAFE_PLAN`.

## 21. CI / Pages

See the final response; filled after the push (CI and Pages status are verified against the pushed commit).

## 22. Architecture Risks

1. result-policy-v1 refuses on the point estimate (22/27 refusals fit). Safe, but loses answers that previously ran on DuckDB.
2. DuckDB out of memory: 38 of 156 DuckDB runs (messages "Out of Memory Error ... This limit was set by ..."); the harness labels them `limit_exceeded` (any message containing "limit"). 14 planned DuckDB choices hit it (9 with a liftable PostGIS that completed, 5 where PostGIS refused the projected column `names`). Not covered by any timeout model or by the row cap: a materialization-memory risk of the default DuckDB budget.
3. A risk model blind to near_count with a large right side (`rp_pool-1086`, P = 0).
4. The metric semi-join classes stay limited; the harness `limit_exceeded` label is ambiguous (row cap vs memory).

## 23. Remaining P0

None.

## 24. Remaining P1

* `result-policy-v2`: optimistic-bound refusal, uncertain band, new fresh set.
* DuckDB memory-budget risk: classify OOM separately in the harness, model it or raise/derive the budget, route to PostGIS when the result is large.
* Risk model features for `near_count` and for P 0.08-0.10; do not re-search the constant.
* Streaming / pagination of large results (explicitly out of scope).
* Rust 256-cell histogram with a regret test, if metric_small matters.

## 25. Recommended Next Phase

DuckDB memory/materialization risk + `result-policy-v2`, validated on one fresh seed with the same freeze-then-run discipline; do not touch the threshold again until a feature (not a constant) changes.
