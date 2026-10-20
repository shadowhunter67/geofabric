# ADR-013 — PostGIS as an execution engine (the first engine family other than DuckDB)

Status: Accepted as a DESIGN and an offline-verified implementation (2026-10-20). **Not verified against a live PostgreSQL/PostGIS server** — none was available and the ~440 MB download was declined. Every claim below says whether it is verified by tests without a database, or unverified.

## Context

Until now GeoFabric had two execution *locations* of one engine family (DuckDB in the gateway, DuckDB-Wasm in the browser). ADR-008 accepted the abstraction (descriptor + executor + calibrated planner) with the explicit scope limit "replaceability across engine families is not covered". PostGIS is the first engine that shares nothing with DuckDB: a different SQL dialect, a different type system, data that lives in a database and not in a file, a connection model, server-side cancellation.

## Decision

`crates/geo-engine-postgis` implements the existing contract — `EngineDescriptor` (id, capabilities, estimate, explain) and `ExecutionEngine::execute` — and is registered in the composition root (`geo-runtime`) exactly like DuckDB. There is no endpoint, flag or code path that runs PostGIS outside `PlanGraph → Planner → EngineDescriptor → ExecutionEngine → metrics`. The architecture fitness check (`tools/arch_check.py`) enforces it: the PostGIS adapter may not depend on the DuckDB family, the DuckDB/browser adapters may not depend on a PostgreSQL driver, the planner may not depend on any engine, the gateway may not use a driver directly.

PostGIS is **optional**: no configuration → no engine, no probe, no error (verified). Enabled by `GEOFABRIC_POSTGIS_ENABLED`, `GEOFABRIC_POSTGIS_URL` (credentials, never logged: `Debug` redacts), `GEOFABRIC_POSTGIS_SOURCES` (the source catalog), `GEOFABRIC_POSTGIS_POOL`.

### Source locality (`LogicalDataset → PhysicalSource[]`, minimal form)

Plans keep naming the *logical* source (the same uri as for GeoParquet). A small catalog (`postgis-sources.json`) says "a copy of it is resident in `schema.relation`, geometry column, SRID, CRS, columns". The trait gained `EngineDescriptor::source_residency(plan)`: the planner asks every engine whether it can reach the plan's data **before scoring**; PostGIS answers by running the (pure) lowering, so "not resident", "CRS differs", "column not stored", "unsupported operation" are all planning-time rejections with a reason, never a SQL error from a server (verified). Importing a file into the database is never a query cost and never a candidate. Limitation: the planner still reads the Parquet footer of the logical file, so a dataset that exists *only* in PostGIS cannot be planned yet (needs a source-metadata provider abstraction: P1, no RFC needed until it is designed).

### Capability matrix (declared in the descriptor; asserted against the lowering by a test)

Supported: Source, Filter, BboxFilter, Intersects, Transform, SpatialSemiJoin, Aggregate, Project, Limit, Sort. Refused with a reason: Buffer, SpatialJoin (pair-producing joins), render sampling (`HashBucket`). The IR is unchanged (0.3).

### CRS and metric semantics

* `EPSG:n → SRID n`; `OGC:CRS84 → SRID 4326` with the axis order **stated**: GeoFabric's external order is (lon, lat) and a PostGIS geometry in SRID 4326 stores x = lon, y = lat. They stay different CRSs at the semantic layer: a relation declared EPSG:4326 does not satisfy a plan that says OGC:CRS84, and vice versa (no implicit cast). Undefined/unknown CRS is an error.
* `Transform` is `ST_Transform(ST_SetSRID(g, from), to)` into a *new* column; the metric predicate is `EXISTS (… ST_DWithin(l.g_metric, r.g_metric, $d))` in the UTM plane; **no `geography`** (rejected: it changes the semantics — ellipsoidal distance — and was not benchmarked or reviewed); degrees are never metres (the lowering refuses an angular distance and a metric predicate on a non-local-metric CRS). Same semantics as the DuckDB path; equivalence on real data is NOT verified.
* Differences PostgreSQL would introduce silently are neutralized: `NULLS LAST` on both directions (DuckDB's default), `COLLATE "C"` on text sort keys (byte order; the cluster is created with the C locale), `ST_AsBinary(g, 'NDR')`, `count_by` tie-break on the group key (ADR-010 follow-up), explicit casts on every bind parameter, refusal of comparisons that would need a type coercion.

### Security

Identifiers come only from the validated catalog (`[A-Za-z_][A-Za-z0-9_]{0,62}`); a column named by a plan must exist in the catalog's relation or the plan's schema, otherwise it is refused. **Values are never SQL text**: every literal, bbox coordinate, WKT, limit and distance is a bind parameter (verified by tests with hostile strings and identifiers, and by parsing every generated statement with a real PostgreSQL grammar). `execute` authorizes every source through the context's access guard **before** consulting the catalog or the database; the probe reads system catalogs only. The runtime role is meant to be `SELECT`-only (`gf_reader`); bootstrap/import use `gf_admin`. The database listens on 127.0.0.1 only; the browser never talks to it.

### Execution: timeout, cancellation, pool, health

`PgConnector` is the only piece that talks to a server. The `live` feature provides the driver: bounded pool; `SET statement_timeout` = the context's execution timeout; a watcher thread issues PostgreSQL's cancel request when the context is cancelled or the deadline passes (a real server-side stop). **This driver is compile-checked only; its behaviour on a server is unverified** (acceptance tests are written in `tests/live.rs` and skip without `GEOFABRIC_POSTGIS_URL`). Errors are mapped by SQLSTATE to the existing taxonomy (timeout, cancelled, transient connection, capability); the runtime now records an execution failure under the engine that actually failed (it was hard-coded to DuckDB), so health blames PostGIS for PostGIS (verified with a mock: three connection failures make it Unhealthy and the planner routes around it). Fallback stays bounded and unchanged: the browser falls back to the selected server engine once; a failing server engine fails clearly.

### Planner participation and cost

The descriptor reports `calibration() = Unmeasured`: there is no measurement of this engine on this system, so its cost is the DuckDB server model used as a **stand-in** (labelled in the candidate's reasons) and its confidence is halved. Policy **R5d**: an unmeasured engine is never chosen on predicted cost while a measured engine can run the plan; an explicit `preferred_engine` or being the only feasible engine selects it. (Verified: with PostGIS registered first so that equal costs tie to it, the planner still returns DuckDB and records R5d.) A cost model v0 with real numbers is deliberately NOT invented: that needs measurements (next phase, with a live database). R5c (no automatic browser above 128 MiB) is unchanged.

### Provenance of metrics

`elapsed_ms` and `rows_returned` are `EngineObserved`; `rows_scanned_upper_bound` is `SourceEstimated` (catalog `row_estimate`); `bytes_read`, `peak_memory_bytes` are `Unknown` (PostgreSQL gives no trustworthy number through this path); columns the relation does not store (e.g. the GeoParquet `bbox` covering struct) are reported in `notes`.

## Not done / not verified (the honest list)

No PostgreSQL ran: no data import, no parity check, no index evidence (`EXPLAIN`), no cross-engine digests, no Demo B on PostGIS, no benchmarks, no measured cost model, no live cancellation/timeout, no live health. The import scripts (`tools/postgis_portable.py`, `scripts/postgis/load_data.py`) were written, not run. `SpatialJoin` and `Buffer` are not implemented.

## Debt

Canonical lowering (RFC-004) is still not realized: the PostGIS lowering is a third, independent compiler of the same IR (Rust/DuckDB, TypeScript/browser, Rust/PostGIS). It was kept small and tested against the same semantics, but the right fix is shared "SQL intent" with dialect printers — the next lowering phase.

## Alternatives rejected

* DuckDB's `postgres` extension / ATTACH: PostGIS would be a DuckDB *source*, proving nothing about engine diversity.
* `geography` for metric distance: see above.
* A special gateway endpoint for PostGIS: violates the one-path invariant.
* Importing files into PostGIS on demand as part of a query: hides a minutes-long cost inside "execution".
