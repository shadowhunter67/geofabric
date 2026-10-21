# Architecture: the PostGIS engine under the engine abstraction

```
Browser ──► Gateway ──► Runtime (composition root) ──► Planner ──► chosen EngineDescriptor ──► ExecutionEngine
                                  │                       │                                         │
                                  │                       │ feasibility (per candidate, BEFORE scoring):
                                  │                       │   capability matrix (ops, formats)
                                  │                       │   source_residency(plan)   ← PostGIS: catalog + lowering dry-run
                                  │                       │   limits / health / probe
                                  │                       │ score (cost model) + confidence + policy R5b / R5c / R5d
                                  │                       ▼
                                  │            PhysicalPlan { candidates[], policy[], … }
                                  ▼
                       PostGisEngine.execute:  validate → AUTHORIZE sources → lower (pure) → cancel check → PgConnector.query → map errors → metrics
                                                                                                                 │
                                                                              (mock in tests | LiveConnector with `postgis-live`) ──► PostgreSQL/PostGIS
```

* Nothing outside this chain runs PostGIS. `tools/arch_check.py` fails the build if the PostGIS adapter depends on the DuckDB family (or the reverse), if the planner depends on any engine, or if the gateway uses a driver.
* The abstraction needed exactly **two additions** to the engine contract, both with defaults so the other engines were untouched: `source_residency(plan, ctx)` (a database engine can only run plans whose data it holds) and `calibration()` (`Measured`/`Unmeasured`: an engine whose cost has not been measured here is explained, not trusted). Everything else — capabilities, estimate, explain, execute, `Metric<T>` provenance, error taxonomy, health, fallback — is reused as is.
* Source locality is explicit: **file-backed** (GeoParquet read by DuckDB), **database-resident** (PostGIS catalog), **browser-accessible** (DuckDB-Wasm over HTTP range requests). The same logical dataset may have several physical locations; each engine advertises which it can reach. Today PostGIS residency is a JSON catalog keyed by the logical uri; a dataset only in the database (no file) is not plannable yet because planning reads the Parquet footer (P1).
* The runtime now records a failed execution under the engine that failed (it blamed DuckDB for everything before): a PostGIS outage degrades PostGIS only.
* Cost: PostGIS has **no measured cost model**. It uses the DuckDB server model as a labelled stand-in with halved confidence, and R5d keeps any measured engine ahead of it. Live measurements now exist (REPORT-GEOFABRIC-POSTGIS-LIVE-VERIFICATION.md) and show the ranking flips with the workload (index-selective windows: PostGIS ahead; large metric semi-joins: DuckDB ahead by ~10x), so one constant factor would be wrong: the model needs selectivity and the semi-join shape as inputs. Not built yet.
* Live-verified contract points: BboxFilter = covering-box overlap (PostGIS stores the covering columns; `&&` on a functional GiST is only an assist); a semi-join's inner side omits that assist; `coverage` guards partial copies; the sync driver runs on blocking threads.
* Lowering debt: three independent compilers of the IR now exist (DuckDB/Rust, browser/TypeScript, PostGIS/Rust). Equivalent semantics are protected by tests and by naming the PostgreSQL defaults that differ (`NULLS LAST`, `COLLATE "C"`, WKB endianness, casts, no coercions), but the real remedy is a shared SQL-intent layer with dialect printers (RFC-004's direction).
