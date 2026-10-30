# ADR-016: Result-delivery policy (`result-policy-v1`)

Status: accepted (2026-10-30). Related: ADR-008, ADR-012, ADR-015 (addenda 2-4), RFC-003 (R5e).

## Problem

The 1,000,000-row cap (`geo_executor::limits::DEFAULT_MAX_ROWS`) was applied by the planner as a per-engine limit of PostGIS only (`max_result_rows`), so a raw result above the cap made PostGIS "infeasible". That mixed two different questions:

* **execution risk** - will this engine finish inside the timeout?
* **delivery risk** - can the FULL result be materialised and handed on, whichever engine computes it?

Consequence: when DuckDB was at `timeout_risk` and PostGIS was "infeasible" only because of the cap, R5e could not lift the block and the planner chose DuckDB, a plan that was either going to time out or to be refused at runtime. DuckDB itself declares no cap, but the runtime enforces `ExecutionContext.max_rows` after execution (`LimitExceeded`) for every engine; the gateway uses 100,001.

## Decision

1. A separate, engine-neutral **result-delivery verdict** (`geo_planner::result_policy`, version `result-policy-v1`) computed from the plan's OUTPUT only: rows, pessimistic rows, bytes, `bounded_by_limit`, against the cap (`min` of the runtime `max_rows` and any server engine's declared `max_result_rows`).

| verdict | meaning |
|---|---|
| `safe` | even the pessimistic row estimate is within the cap |
| `bounded` | an explicit, certain user `Limit` within the cap shortens the result |
| `uncertain` | the point estimate fits, the pessimistic bound does not (reported as `Assessment.uncertainty`) |
| `too_large` | the point estimate of the FULL result is above the cap: no server engine can deliver it |

2. `too_large` is a plan-time refusal for every server engine and surfaces as `NO_SAFE_PLAN (the full result cannot be delivered): ...` with only semantically valid alternatives (add a filter, reduce the area, add an explicit limit, aggregate/count). It is not an engine choice and not an execution risk.
3. **Invariants.** Aggregates over more than 1M matches stay legal (only OUTPUT rows count). A raw return above the cap stays blocked (also at runtime by `LimitExceeded`). "Return all matching rows" is never silently cut to "first N". Render sampling (ADR-012) is a rendering concern and does not change analytical results. `limit_certain` is respected: an uncertain limit is not a guarantee.
4. The policy reports it, it does not hide it: `Assessment.result_delivery` carries version, verdict, rows, rows_high, bytes, cap and reason; developer mode shows it, Explorer shows one short sentence for `NO_SAFE_PLAN`.
5. `metric-semijoin-policy-v1.1` = R5e plus the delivery uncertainty note. R5e itself is unchanged.

## Evidence (confirmation, seed 20261501, selection 20261502, 156 plan-time-stratified cases)

0 cap violations, 0 DuckDB/PostGIS mismatches, 27 `NO_SAFE_PLAN` (5 truly above the cap or unrunnable, 22 would in fact have fit: actual 144k-937k rows). See `REPORT-GEOFABRIC-RESULT-RISK-POLICY.md`.

## Known limitation (P1: `result-policy-v2`)

v1 judges the POINT estimate. The estimator is biased upward (planned feature cases: estimate / actual median x1.17, P90 x1.52, max x19; the pessimistic bound covered the actual in 106/106), so 22 of 27 refusals refused a deliverable result, including the fixed regression `thr_pool2-0726` (estimate 1,095,005, PostGIS returned 925,879). A v2 should refuse only when the optimistic bound is above the cap and treat the band between as `uncertain` (the runtime cap still blocks a true >1M result). It was NOT changed in this phase: tuning on the confirmation set is not allowed, v2 needs its own fresh set.

## Not in scope

Streaming or pagination of large results (P1). New engines. Changing the cap value.
