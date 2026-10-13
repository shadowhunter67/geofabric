# Actual execution pipeline (what the code really does)

```
Spatial IR (engine-neutral, CRS + units explicit)
   │  SourceAccessGuard.authorize(every source)      ← before ANY metadata I/O
   ▼
Planner v0 (reads Parquet footers of authorized sources only; rule-based; decision log)
   ▼
DuckDB adapter: IR → SQL (private to the adapter), sandboxed connection, watchdog (timeout/cancel)
   ▼
DuckDB reads the Parquet file itself (its own reader, its own pruning)
   ▼
geometry leaves DuckDB as WKB (ST_AsWKB)
   ▼
GeoFabric re-attaches GeoArrow extension metadata + CRS taken from the IR schema
   ▼
Arrow RecordBatches  ──►  gateway: WKB → GeoJSON at the HTTP boundary only  ──►  MapLibre
```

## Where the plan runs (multi-engine)

```
Spatial IR (PlanGraph, 0.2)  ──►  Planner v1: feasibility (ops, memory guard, covering) + calibrated score
        │                                   │
        │                                   ├─ selected = DuckDB (server)  →  execute in-process  → Arrow → gateway → UI
        │                                   └─ selected = DuckDB-Wasm      →  Dispatch::Client: the gateway returns the PlanGraph +
        │                                          authorized source URLs; the browser fetches Parquet byte ranges and runs
        │                                          the same plan in a Web Worker (TypeScript executor, same canonical digest)
        └─ if the browser engine fails: re-planned with the browser disabled (fallback is shown in the UI)
```

## What is **not** true (do not claim it)

- **Not zero-copy end-to-end.** Parquet → DuckDB buffers → Arrow (DuckDB conversion) → gateway → GeoJSON: several copies/conversions. None has been measured except the GeoJSON boundary conversion.
- **GeoFabric's Arrow reader does not control execution.** `GeoParquetSource::read` (Arrow-native, projection + row-group selection) is used for metadata, oracle tests and tooling; the query itself is read by DuckDB. P1 architecture debt (accepted temporarily).
- **Row-group pruning is not observed.** `row_groups_skipped` is GeoFabric's analysis of footer statistics (`SourceEstimated`). DuckDB prunes by itself; the identical outcome is inferred from timing, not reported by DuckDB.
- **Actual bytes read is unknown.** DuckDB's profiler counter under-counts by orders of magnitude and is not reported.
- **The logical plan is a DAG (PlanGraph, IR 0.2)** — but DuckDB still evaluates a shared node once per consumer (shared nodes compile to `NOT MATERIALIZED` CTEs); multi-stage / multi-engine *splitting* of one graph is not implemented.
- **Two real executors, one engine family.** DuckDB (server) and DuckDB-Wasm (browser) both run the same plans with equal digests, but both are DuckDB: replaceability across *different* engine families (e.g. PostGIS) is not demonstrated.
- **The browser engine does not prune row groups through struct statistics** (measured): it reads every bbox covering column chunk of the file, which the planner models explicitly.

Metric provenance, authorization boundary and timeout semantics: [RFC-002](../rfcs/RFC-002-execution-engine-contract.md).
