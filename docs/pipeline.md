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

## What is **not** true (do not claim it)

- **Not zero-copy end-to-end.** Parquet → DuckDB buffers → Arrow (DuckDB conversion) → gateway → GeoJSON: several copies/conversions. None has been measured except the GeoJSON boundary conversion.
- **GeoFabric's Arrow reader does not control execution.** `GeoParquetSource::read` (Arrow-native, projection + row-group selection) is used for metadata, oracle tests and tooling; the query itself is read by DuckDB. P1 architecture debt (accepted temporarily).
- **Row-group pruning is not observed.** `row_groups_skipped` is GeoFabric's analysis of footer statistics (`SourceEstimated`). DuckDB prunes by itself; the identical outcome is inferred from timing, not reported by DuckDB.
- **Actual bytes read is unknown.** DuckDB's profiler counter under-counts by orders of magnitude and is not reported.
- **The IR is a tree**, not a DAG (see RFC-001: current representation vs `PlanGraph` target).
- **One real engine.** The second-engine property is demonstrated only with a mock in tests.

Metric provenance, authorization boundary and timeout semantics: [RFC-002](../rfcs/RFC-002-execution-engine-contract.md).
