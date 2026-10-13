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
