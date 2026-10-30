# RFC-003 — Query Planner

Status: Proposed (v1 implemented 2026-10-13: PlanGraph input, multi-engine selection, explanations)

## Contract

```
Planner::plan(PlanGraph, SourceMetadata (via SourceAccessGuard), [EngineDescriptor]) -> PhysicalPlan
PhysicalPlan { engine, placement(server|browser), stages[], candidates[], scans[], estimate,
               input_bytes_upper_bound, result_rows_estimate, decisions[], steps[] }
```

The planner never executes anything and never names an engine: engines arrive as `EngineDescriptor` (capabilities, limits, estimate, explain). In-process engines additionally implement `ExecutionEngine::execute`; a client engine (the browser) is only a descriptor on the Rust side and is executed by its own host.

`stages` is a list so that multi-engine plans can be expressed later; v1 always emits one stage (single-engine plan). Splitting a graph across engines is **not implemented**.

## Steps

1. Validate the graph (structure, CRS, units).
2. Scan analysis over the graph: row-group constraints are pushed to a source only through row-preserving operators; a shared source gets the **union** of its access paths. Sources are authorized before any footer is read.
3. For each engine: **feasibility** (hard limits) then **score**.
4. `preferred_engine` hint wins if feasible, otherwise ignored with a warning.
5. Lowest score wins; ties go to the first registered engine.
6. Cost guard (`max_scan_bytes`), then engine estimate/explain.

## Feasibility (never traded against score)

operations supported; source format supported; browser enabled (`allow_browser`); estimated input ≤ engine limit and ≤ `browser_max_input_bytes` (memory guard); estimated result rows ≤ limit; `bbox_filter_requires_covering` engines need a bbox covering column on every source (declared or inferred).

## Cost model (all parameters configurable: `PlannerConfig.cost`)

```
score_ms(server)  = server_fixed_ms  + input_bytes / server_scan_rate  + result_bytes / network_rate
score_ms(browser) = browser_fixed_ms + input_bytes / browser_scan_rate + input_bytes  / network_rate
```

`input_bytes` = Parquet statistics upper bound after pruning; `result_bytes` = estimated result rows × `avg_result_row_bytes` (result rows from the root operator: aggregate → 1 or `aggregate_result_rows`, limit → n, else input rows). Server engines scan next to the data and ship the **result**; the browser must pull the **input**. The model is linear and deliberately naive; the defaults are calibrated from the crossover benchmark (see the multi-engine report) on a machine where network transfer is nearly free — the network term is an assumption, not a measurement.

## Explainability

Every candidate carries `feasible`, `score_ms`, a cost breakdown, reasons and rejection reasons; the decision log records validation, pruning (with "covering inferred" warnings), candidates, hints, ties and the final selection with its alternatives. The UI renders this verbatim.

## Not implemented

cost model beyond linear, cache/locality, multi-stage plans, learned estimates, remote object storage.

## v1.1 hardening (2026-10-14; ADR-009, ADR-010)

`Planner::plan_with_inputs(plan, engines, ctx, &PlanInputs)`; `plan()` = the same with an empty world. `PlanInputs`: runtime probes, health views, known incompatibilities, calibration, network profile, browser warm/cold. `PhysicalPlan` gains `assessment` { confidence (0–1 with 7 reasoned factors), engine confidence, margin factor, estimate (rows, interval, quality), network, health, probes, planned fallback (max 1), browser access strategy per source (`full` | `range`), policy notes } and each candidate gains `confidence`, `risk`, `health`, `probe_status`. New feasibility rules: probe failure / unverified operation / unreadable file class / failed range requests, unhealthy engine, known incompatibility, browser memory risk (replaces the fixed `browser_max_input_bytes`, now an optional operator override). New selection rule R5b (confidence policy). `browser_max_input_bytes` is `Option<u64>`; `CostModel.network_mb_per_s` is gone (network = `NetworkProfile`).

## Engine cost models and class-scoped R5d (2026-10-23; ADR-015)

Cardinality estimation and engine cost are separate concerns. The planner derives `WorkloadFeatures` (plan-time facts: matching rows, result rows/bytes, operation flags, limit, semi-join probes/right rows/metric/distance) from the plan, the source scans and the row estimator, and hands them to each engine's optional `cost_estimate`. An engine answers with a model version, a **workload class**, a **validation level** (`validated | limited | unvalidated`), time and terms, a **hard risk verdict** and the index state it assumed. Order of decisions: authorization, source availability (residency), capabilities, health, **hard risk policy** (a blocked workload is infeasible for automatic selection; an explicit hint overrides it), then cost, then **R5d, scoped by class**: a candidate is "measured" only when its engine's model is `validated` for this plan's class with an `available` index; otherwise the engine keeps its engine-level calibration. The planner never inspects a model's insides and `geo-plan` / the canonical lowering know nothing about any of it. `Candidate.cost_model` carries the verdict to the Developer-mode explanation.

## Risk states, deferred blocks and uncertainty: policy `metric-semijoin-policy-v1` (2026-10-26; ADR-015 addendum 2)

"The other engine is not validated" is a statement about evidence for THAT engine; it says nothing about the engine R5d prefers. Two held-out cases made this concrete: R5d kept DuckDB, DuckDB hit its 60 s timeout, PostGIS finished in 12-18 s. The planner therefore keeps cost, validation and risk apart for every engine that supplies a model verdict:

* **Risk level** (`EngineCostEstimate.risk.level`): `low | elevated | timeout_risk | unknown | high`. `timeout_risk` = the engine's own model puts P(exceeding the execution timeout) at or above the engine's threshold; `unknown` = the model lacks a plan-time feature or the plan is outside its measured envelope (never a synonym of `low`); `high` is the cost-based PostGIS repeated-probe label. `timeout_probability` carries the number. The model is a log-linear Tobit fit (timeouts censored) restricted to the regime where timeouts occur; below the regime the verdict is `low` without a probability.
* **Deferred block** (`blocked` + `overridable`): a block on COST grounds (PostGIS over 50,000 probes) makes the candidate infeasible but keeps it scored (`Candidate.deferred_by_risk`). A block that is not overridable (capability, residency, a hard safety rule) is never lifted.
* **R5e** (after R5d): if the selected candidate is at `timeout_risk`, choose the cheapest feasible candidate that is not; if only a deferred candidate is not at risk, lift its block for that reason alone and say so; if every option is at risk, take the one least likely to time out and attach `Assessment.uncertainty`. A selected candidate with `unknown` risk also yields an uncertainty note and keeps its choice. An explicit engine hint bypasses R5e (it is the user's decision; the warning is still shown).
* **R5f - result delivery** (`result-policy-v1`, ADR-016; separate from execution risk, engine-neutral): the plan's OUTPUT rows/bytes are judged against the delivery cap (`ExecutionContext.max_rows`, tightened by any server engine's declared `max_result_rows`) as `safe | bounded | uncertain | too_large`. `too_large` removes every server engine and the planner answers `NO_SAFE_PLAN (the full result cannot be delivered)` with valid alternatives (filter, smaller area, explicit limit, aggregate/count); `uncertain` is reported in `Assessment.uncertainty`. Aggregates over more than the cap stay legal, a raw result above the cap stays blocked, nothing is ever silently cut to "first N", render sampling is not part of it. An explicit engine hint still bypasses R5e but NOT the runtime cap.
* **Never**: speculative dual execution, a retry on the same engine, a second fallback. R5e only changes WHICH single engine is planned.

The model versions are separate from the policy version: cost `postgis-cost-v3` / `duckdb-semijoin-cost-v3`, risk `duckdb-timeout-risk-v1` / `postgis-timeout-risk-v1`, policy `metric-semijoin-policy-v1`. The planner log (decisions + `policy`) carries the verdicts and, for metric semi joins, the plan-time candidate-work features (`PhysicalPlan.workload`).
