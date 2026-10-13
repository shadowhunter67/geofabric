# RFC-002 — Execution Engine Contract

Status: Proposed (revised 2026-10-13 during hardening: metric provenance, authorization, timeout)

```
trait ExecutionEngine: Send + Sync {
  fn id() -> EngineId
  fn capabilities() -> Capabilities
  fn estimate(plan, ctx) -> Estimate          // metadata-based upper bounds, each with provenance
  fn explain(plan, ctx)  -> Vec<ExplainStep>  // what will actually run
  fn execute(plan, ctx)  -> ArrowBatches + ExecutionMetrics
}
```

## Rules

- The engine receives a *validated* Spatial IR and translates it privately (SQL for DuckDB/PostGIS). Translation only from validated IR, parameterised/escaped, never from raw user strings.
- Result: Arrow `RecordBatch`es; geometry as WKB tagged with GeoArrow extension metadata whose CRS comes **from the IR schema**, not from the engine.

## Authorization boundary (new)

`ExecutionContext.guard: Arc<dyn SourceAccessGuard>`. Every source URI goes through `guard.authorize(uri)` *before* any I/O on it (footer read, existence check, scan). `authorize` returns an `AuthorizedSource` (constructible only by the guard) — the type is the proof. Denials are opaque (`source not permitted`): no path, no reason, no existence oracle. Planner, runtime and engines all use the same guard; engines additionally confine their own engine (DuckDB: `allowed_paths` = the authorized files only, external access disabled).

## Metrics contract (new)

Engines are **not** required to supply every metric and must never invent one.

```
Metric<T> { value: Option<T>, source: MetricSource }
MetricSource = EngineObserved | SourceObserved | SourceEstimated | PlannerEstimated | Derived | Unknown
```

| metric | provenance in the DuckDB adapter |
|---|---|
| `elapsed_ms`, `rows_returned` | EngineObserved |
| `row_groups_total` | SourceObserved (Parquet footer) |
| `row_groups_skipped` | SourceEstimated (GeoFabric analysis of footer statistics; DuckDB does not report its pruning) |
| `rows_scanned_upper_bound`, `estimated_bytes_upper_bound` | SourceEstimated |
| `bytes_read` | **Unknown** (`value: None`): DuckDB's profiler counter under-counts by orders of magnitude |
| `peak_memory_bytes` | EngineObserved (buffer-manager peak only) |

UIs must show the provenance next to the value (Observed / Estimated / Derived / Unavailable) and must not render an estimate as an observed fact.

## Limits, timeout, cancellation (new)

`ExecutionContext` carries `max_rows`, `memory_limit_mb`, `threads`, `timeout` and a `CancelToken`. Defaults live in `geo_executor::limits` (single place; default query timeout 30 s). The **engine** enforces them: `Timeout { limit_ms }` / `Cancelled` mean the running query was interrupted (DuckDB adapter: a watchdog calls `InterruptHandle::interrupt()`), not that a caller stopped waiting. A client disconnect cancels the token (gateway).

## Engine #2 check

A `MockEngine` in the planner tests runs the same plans without any change to IR or planner. No second *real* engine exists yet.
