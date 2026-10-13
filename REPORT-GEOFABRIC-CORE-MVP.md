# REPORT — GeoFabric Core MVP

Prepared for independent Principal Architect review. Date: 2026-10-13. All numbers come from one Windows 11 machine (28 logical cores, 32 GB RAM, Rust 1.99, DuckDB 1.5.6). No claim here is based on a second machine.

## 1. Executive Summary

**Built and working (verified by tests or runs):**
- `arch_check.py` (dependency-graph fitness rules), `geo-types`, `geo-ir` (10 node kinds, schema derivation, validation, versioned JSON), synthetic GeoParquet generator, `GeoParquetSource` (footer metadata, CRS, row-group stats, bbox pruning, Arrow reads), DuckDB execution adapter, rule-based planner v0 with a decision log, a runtime that wires them, an HTTP gateway and a web UI (MapLibre) that shows the execution plan.
- Vertical slice GeoParquet → IR → validate → plan → DuckDB → Arrow runs end to end; 44 tests pass; benchmarks at 10 MB, 100 MB and 1 GB (synthetic) are reproducible and recorded.
- Pruning is real: with spatially ordered row groups a 0.6 % viewport skips 61 of 65 (100 MB) and 126 of 130 (1 GB) row groups and runs 1.5×–46× faster (depending on query and size) than the same data in shuffled order, with identical results on the 10 MB oracle tests.

**Not built / not verified:** WASM and PostGIS engines, a cost model, Buffer *execution* tests (no projected-CRS dataset), CRS transform, per-query timeout, authentication, CI wiring, raster/AI/versioning. In the browser only the load → run → execution-plan path was checked; the feature-click Inspector and the Results tab were **not** verified visually.

**MVP status:** core slice is functional and reviewable; it is **not** production-ready. Treat the IR and engine contract as Proposed (RFC/ADR statuses are still `Proposed`).

**Assessment:** the central thesis (engine-neutral IR + planner that can explain a pruned scan) holds on synthetic data. The weakest evidence is (a) real-world data (only synthetic rectangles were used), (b) measured I/O (DuckDB's byte counter is unusable, see §13), and (c) the "second engine" claim, which is only exercised by a mock in unit tests.

## 2. Repository State

- Branch: `main`; commit: {{COMMIT}} (earlier: `d66684a` docs, then incremental commits per milestone).
- Technologies: Rust workspace (arrow/parquet 58, duckdb crate 1.10506 bundled, axum 0.8, serde), Python 3.12 (tooling only), TypeScript 7 + Vite 8 + MapLibre GL 6.
- Tree:

```
CONSTITUTION.md AGENTS.md CLAUDE.md README.md REPORT-GEOFABRIC-CORE-MVP.md
adr/ADR-001..006   rfcs/RFC-001..003   docs/
crates/geo-types  geo-ir  geo-executor  geo-source-geoparquet  geo-planner
       geo-engine-duckdb  geo-runtime  geo-gateway  geo-bench
web/ (TS UI)   benchmarks/ (generator, results)   tools/arch_check.py
```

## 3. Architecture Implemented

```
web (TS/MapLibre) → geo-gateway (HTTP; GeoJSON only here)
                      → geo-runtime → geo-planner ─(trait only)→ ExecutionEngine
                                         │                         ↑
                                         ├→ geo-source-geoparquet  geo-engine-duckdb (IR→SQL private here)
                                         └→ geo-ir → geo-types      geo-executor (contract)
```

## 4. geo-types and Spatial IR

**geo-types.** `Crs {Epsg(u32), Unknown(String)}` (no `Default`; absent/unparseable stays `Unknown`), `GeometryType`, `Distance`/`Area` with private fields and a mandatory unit (NaN, ±inf, negative rejected, also on deserialization), `DistanceUnit {Meter, Kilometer, Foot, Degree}` where `Degree` is angular and `to_meters()` errors for it, `AreaUnit`, `BoundingBox` (CRS mandatory; CRS mismatch is an error, no antimeridian handling), `SpatialSchema`. 7 unit tests.

**Spatial IR.** Node kinds: Source, Filter, BboxFilter, Intersects, Buffer, SpatialJoin, Aggregate, Project, Limit, Sort. Per node: operation, inputs, derived output schema (with geometry columns + CRS), content hash (`node_id`, sha256 over version + canonical JSON), lineage (post-order chain), required-capability set (`required_ops`). Source nodes carry optional statistics. Plan JSON: `{"ir_version":"0.1","root":…,"hints":…}`, deterministic (no maps), `deny_unknown_fields`, unsupported version rejected. `Buffer` carries `distance: Distance` (unit inside), `input_crs`, `output_crs`, `error_tolerance`, `output_column`. `SpatialJoin` requires an explicit `right_prefix`.

**Validation** is separate from parse. It rejects: unknown CRS on CRS-sensitive ops; bbox/literal/join CRS ≠ column CRS (no implicit transform); declared `input_crs` ≠ column CRS; metric buffer on a geographic CRS; degree buffer on projected; CRS of unknown kind (valid EPSG but outside the tiny built-in table → refused, not guessed); missing columns; geometry in attribute filters/sort/group-by; non-numeric Sum/Avg; ambiguous join output names. Degree buffer on a geographic CRS is allowed with a warning. 10 integration tests.

IR deviation: a **tree**, not a DAG (RFC-001 says DAG). Hashes make a DAG representation possible later.

## 5. Architecture Fitness

`tools/arch_check.py` parses `cargo metadata` (resolved, transitive; dev-deps and wasm-target-only deps excluded). Rules: geo-types / geo-ir / geo-executor must not (transitively) reach planner, runtime, sources, engines, UI crates or cloud SDKs; geo-planner must not reach engines/UI/cloud; sources must not reach planner/engines/UI; engine adapters must not reach planner/runtime/UI; `geo-gateway` must not depend **directly** on engine crates; `web/package.json` must not contain engine/DB packages. Status: passing (6 crates matched by transitive rules + the direct gateway rule). Negative self-tests (`--extra-forbid=geo-ir=sha2`, `…geo-planner=geo-executor`) correctly fail. **Gaps:** not wired into CI (no CI file exists); "distance must carry a unit" / "CRS explicit" are enforced by Rust types and validation tests, not by this script; there is no check that the planner source never names `duckdb` in text.

## 6. Synthetic Data

`benchmarks/gen_synthetic.py --size 10mb|100mb|1gb`, seed 42. Matrix: distribution {clustered (40 Gaussian clusters, Dirichlet weights), uniform} × ordering {spatial (Morton/Z-order of centroids), shuffled (seeded permutation)}. Rectangles (WKB Polygon, EPSG:4326, extent 100–110°E / 8–24°N), columns id, category (8 values), height, geometry, `bbox` struct covering column, zstd, GeoParquet 1.1-style `geo` metadata with PROJJSON `id` EPSG:4326. Rows: 130 k / 1.3 M / 13 M; row-group size 5 k / 20 k / 100 k. Per-file JSON manifest (seed, rows, row groups, bbox, size, sha256 for files < 300 MB). Reproducibility verified: two generations of `uniform_10mb_spatial` give the identical SHA-256. Same seed ⇒ spatial and shuffled files hold identical rows (used as a test invariant).

Limit: rectangles only, no real-world skew, no multi-geometry, no nulls.

## 7. GeoParquet Source

Reads: schema (mapped to `FieldType`), `geo` metadata (primary column, encoding must be WKB, geometry types, bbox, covering), CRS (PROJJSON `id`/string), per-row-group rows, compressed bytes per root column, bbox from covering-column min/max statistics. `prune(bbox)` is conservative (groups without stats are kept) and errors on CRS mismatch or unknown source CRS. `read(columns, row_groups)` gives an Arrow `RecordBatch` reader with column projection and row-group selection (no GeoJSON). `analyze_plan(plan)` pushes BboxFilter/Intersects (WKT envelope) constraints down **only through row-preserving operators** (Filter, Sort, Project, BboxFilter, Intersects); Limit/Aggregate/Buffer/Join reset constraints. 8 tests + a test that spatially ordered files prune strictly more than shuffled ones.

**CRS decision to review:** an absent `crs` in GeoParquet metadata means OGC:CRS84 per spec; the source reports `Unknown("OGC:CRS84 (crs absent …)")`, so such files cannot be bbox-filtered until a CRS is declared. This is stricter than the spec on purpose (Constitution 2).

**Limitations:** WKB only (no GeoArrow-native encodings); remote (HTTP/S3) sources not implemented; predicate pushdown into Parquet exists only via bbox covering statistics (no attribute-column statistics); column-projection byte estimates are upper bounds (`predicted bytes` count all columns); the Arrow reader is used for metadata/oracle tests — **execution reads Parquet through DuckDB, not through this reader** (deviation from "Arrow-native end to end", see §14).

## 8. Arrow / GeoArrow

Results are Arrow `RecordBatch`es. Geometry leaves DuckDB as WKB (`ST_AsWKB`) because exporting DuckDB's native `GEOMETRY` type through the Rust `query_arrow` fails; the adapter then tags the field with `ARROW:extension:name = geoarrow.wkb` and extension metadata whose CRS comes from the **IR schema** (not from DuckDB). Copies: Parquet → DuckDB buffers → Arrow (DuckDB-owned conversion) → gateway; at the HTTP boundary WKB → GeoJSON (hand-written 2D decoder, 3 tests). Measured cost of the boundary conversion for 20 000 polygons in a debug build: ~0.6 s. No zero-copy path exists today; not measured beyond that.

## 9. DuckDB Adapter

In-process DuckDB 1.5.6 via the `duckdb` crate (`bundled`, `parquet`), `spatial` loaded at runtime. IR → SQL is a CTE chain in `sql.rs` (identifiers quoted, literals escaped or numeric, WKT restricted to a safe charset, file paths canonicalised and checked against allowed roots, remote URLs refused). After loading the extension the connection gets `allowed_directories` = data roots + a private temp dir and `enable_external_access=false`, `threads`, `memory_limit`; results are capped by `max_rows`. `capabilities()` reports Intersects/Buffer/SpatialJoin only when `spatial` loaded. `BboxFilter` and `Intersects` add a covering-column prefilter so DuckDB can prune row groups.

Limitations: no `Transform` node, Buffer with CRS change or `error_tolerance` is Unsupported; **Buffer execution is untested** (compile path only reasoned about; metric buffers need a projected dataset which was not generated); no query timeout; each `execute` opens a fresh in-memory DuckDB and re-`LOAD`s spatial (planning + overhead is ~180 ms in a debug build); build/disk findings in ADR-006.

## 10. Planner v0

Rule-based, in `geo-planner`. Inputs: IR, registered engines (trait only), `ExecutionContext`. Rules: R1 validate (errors abort); R2 scan analysis; R3 capability filter (missing ops / source format → rejected, with reason); R4 `preferred_engine` hint honoured or ignored with a warning; tie-break = first registered; R5 cost guard from metadata (warn 512 MB, refuse 8 GB — arbitrary placeholders, not justified by benchmarks). Output `PhysicalPlan {plan_id, engine, scans, estimate, decisions[], steps[]}` — serialisable and rendered by the UI. 5 tests with mock engines prove a second engine plugs in with no change to IR or planner. **Not implemented:** cost model, locality/network/cache, engine selection by data size (no second real engine exists). The "second engine" criterion is therefore only demonstrated with a mock that returns nothing.

## 11. Integration Tests

| test | type | status | notes |
|---|---|---|---|
| geo-types (7) | unit | pass | serde roundtrip, NaN/inf, degree→meters error |
| geo-ir (10) | integration | pass | roundtrip, versions, CRS/unit rejections |
| source (6) + analysis (2) | integration/unit | pass | fixtures written in-test; benchmark-file prune comparison |
| planner (5) | integration | pass | mock engines |
| vertical (11) | end-to-end | pass | bbox/filter/intersects/aggregate vs an **independent Rust oracle** on 10 MB data; spatial-vs-shuffled; injection quote; path escape; max_rows; CRS metadata |
| gateway wkb (3) | unit | pass | hostile counts |

Total 44 passing. Coverage was not measured (no coverage tool run). **Weak spots:** `spatial_join_self_join_runs` only asserts `n >= 0` (it does **not** validate the join result); no test covers Buffer execution; gateway endpoints and the web UI have no automated tests; vertical tests skip silently when benchmark data is absent.

## 12. Benchmark Results

Release build, 3 timed reps after 1 warm-up (OS cache warm), median ms; `predicted bytes` = compressed bytes of the row groups the statistics analysis keeps (all columns, upper bound); DuckDB's own peak buffer memory stayed at or below ~90 MB in every run. Full raw rows: `benchmarks/results/bench-{10mb,100mb,1gb}.{md,json}`.

| size | dist | query | rows ret | rg skipped spatial | rg skipped shuffled | predicted bytes spatial | predicted bytes shuffled | ms spatial | ms shuffled | speedup |
|---|---|---|---|---|---|---|---|---|---|---|
| 10mb | clustered | bbox_count_small(0.6%) | 1 | 23/26 | 0/26 | 1.1 MB | 10.0 MB | 10.1 | 41.1 | 4.1x |
| 10mb | clustered | bbox_count_large(25%) | 1 | 18/26 | 0/26 | 3.0 MB | 10.0 MB | 10.6 | 18.4 | 1.7x |
| 10mb | clustered | attr_filter_fullscan | 1 | 0/26 | 0/26 | 9.8 MB | 10.0 MB | 10.8 | 11.2 | 1.0x |
| 10mb | clustered | intersects_count_small | 1 | 23/26 | 0/26 | 1.1 MB | 10.0 MB | 12.2 | 28.4 | 2.3x |
| 10mb | clustered | aggregate_by_category_small | 8 | 23/26 | 0/26 | 1.1 MB | 10.0 MB | 22.6 | 18.6 | 0.8x |
| 10mb | clustered | spatial_join_small | 1 | 46/52 | 0/52 | 2.3 MB | 20.0 MB | 38.6 | 63.5 | 1.6x |
| 10mb | clustered | return_rows_small(id,geometry) | 33 | 23/26 | 0/26 | 1.1 MB | 10.0 MB | 14.9 | 87.3 | 5.9x |
| 10mb | uniform | bbox_count_small(0.6%) | 1 | 19/26 | 0/26 | 2.7 MB | 10.0 MB | 11.5 | 16.9 | 1.5x |
| 10mb | uniform | bbox_count_large(25%) | 1 | 14/26 | 0/26 | 4.6 MB | 10.0 MB | 14.1 | 16.8 | 1.2x |
| 10mb | uniform | attr_filter_fullscan | 1 | 0/26 | 0/26 | 9.9 MB | 10.0 MB | 11.2 | 10.9 | 1.0x |
| 10mb | uniform | intersects_count_small | 1 | 19/26 | 0/26 | 2.7 MB | 10.0 MB | 10.8 | 38.2 | 3.5x |
| 10mb | uniform | aggregate_by_category_small | 8 | 19/26 | 0/26 | 2.7 MB | 10.0 MB | 13.4 | 18.5 | 1.4x |
| 10mb | uniform | spatial_join_small | 1 | 45/52 | 0/52 | 2.7 MB | 20.1 MB | 22.6 | 70.1 | 3.1x |
| 10mb | uniform | return_rows_small(id,geometry) | 844 | 19/26 | 0/26 | 2.7 MB | 10.0 MB | 21.2 | 88.4 | 4.2x |
| 100mb | clustered | bbox_count_small(0.6%) | 1 | 61/65 | 0/65 | 6.1 MB | 101.1 MB | 8.8 | 43.6 | 4.9x |
| 100mb | clustered | bbox_count_large(25%) | 1 | 48/65 | 0/65 | 25.8 MB | 101.1 MB | 17.1 | 42.4 | 2.5x |
| 100mb | clustered | attr_filter_fullscan | 1 | 0/65 | 0/65 | 98.3 MB | 101.1 MB | 9.7 | 18.4 | 1.9x |
| 100mb | clustered | intersects_count_small | 1 | 61/65 | 0/65 | 6.1 MB | 101.1 MB | 10.1 | 117.2 | 11.7x |
| 100mb | clustered | aggregate_by_category_small | 8 | 61/65 | 0/65 | 6.1 MB | 101.1 MB | 8.4 | 47.0 | 5.6x |
| 100mb | clustered | spatial_join_small | 1 | 123/130 | 0/130 | 10.7 MB | 202.2 MB | 20.3 | 187.2 | 9.2x |
| 100mb | clustered | return_rows_small(id,geometry) | 350 | 61/65 | 0/65 | 6.1 MB | 101.1 MB | 15.9 | 373.6 | 23.5x |
| 100mb | uniform | bbox_count_small(0.6%) | 1 | 55/65 | 0/65 | 15.3 MB | 101.4 MB | 14.2 | 43.4 | 3.1x |
| 100mb | uniform | bbox_count_large(25%) | 1 | 36/65 | 0/65 | 44.3 MB | 101.4 MB | 25.0 | 44.9 | 1.8x |
| 100mb | uniform | attr_filter_fullscan | 1 | 0/65 | 0/65 | 99.1 MB | 101.4 MB | 14.9 | 13.7 | 0.9x |
| 100mb | uniform | intersects_count_small | 1 | 55/65 | 0/65 | 15.3 MB | 101.4 MB | 22.5 | 132.3 | 5.9x |
| 100mb | uniform | aggregate_by_category_small | 8 | 55/65 | 0/65 | 15.3 MB | 101.4 MB | 15.0 | 48.9 | 3.3x |
| 100mb | uniform | spatial_join_small | 1 | 123/130 | 0/130 | 10.7 MB | 202.7 MB | 28.8 | 254.8 | 8.9x |
| 100mb | uniform | return_rows_small(id,geometry) | 8184 | 55/65 | 0/65 | 15.3 MB | 101.4 MB | 43.0 | 388.3 | 9.0x |
| 1gb | clustered | bbox_count_small(0.6%) | 1 | 126/130 | 0/130 | 30.1 MB | 1004.9 MB | 17.8 | 446.4 | 25.1x |
| 1gb | clustered | bbox_count_large(25%) | 1 | 97/130 | 0/130 | 246.4 MB | 1004.9 MB | 107.8 | 453.0 | 4.2x |
| 1gb | clustered | attr_filter_fullscan | 1 | 0/130 | 0/130 | 968.5 MB | 1004.9 MB | 51.4 | 89.4 | 1.7x |
| 1gb | clustered | intersects_count_small | 1 | 126/130 | 0/130 | 30.1 MB | 1004.9 MB | 26.1 | 1166.0 | 44.7x |
| 1gb | clustered | aggregate_by_category_small | 8 | 126/130 | 0/130 | 30.1 MB | 1004.9 MB | 16.8 | 465.1 | 27.7x |
| 1gb | clustered | spatial_join_small | 1 | 253/260 | 0/260 | 52.7 MB | 2009.9 MB | 48.8 | 2129.9 | 43.7x |
| 1gb | clustered | return_rows_small(id,geometry) | 3507 | 126/130 | 0/130 | 30.1 MB | 1004.9 MB | 82.0 | 3801.6 | 46.3x |
| 1gb | uniform | bbox_count_small(0.6%) | 1 | 118/130 | 0/130 | 90.3 MB | 1007.2 MB | 54.7 | 440.2 | 8.1x |
| 1gb | uniform | bbox_count_large(25%) | 1 | 81/130 | 0/130 | 368.7 MB | 1007.2 MB | 187.4 | 448.9 | 2.4x |
| 1gb | uniform | attr_filter_fullscan | 1 | 0/130 | 0/130 | 977.6 MB | 1007.2 MB | 45.2 | 95.1 | 2.1x |
| 1gb | uniform | intersects_count_small | 1 | 118/130 | 0/130 | 90.3 MB | 1007.2 MB | 53.8 | 1223.0 | 22.7x |
| 1gb | uniform | aggregate_by_category_small | 8 | 118/130 | 0/130 | 90.3 MB | 1007.2 MB | 32.9 | 283.2 | 8.6x |
| 1gb | uniform | spatial_join_small | 1 | 251/260 | 0/260 | 67.7 MB | 2014.5 MB | 94.9 | 1324.8 | 14.0x |
| 1gb | uniform | return_rows_small(id,geometry) | 81698 | 118/130 | 0/130 | 90.3 MB | 1007.2 MB | 219.2 | 2602.7 | 11.9x |


Columns: `rows ret` = rows returned (count queries return 1); counts themselves are **not** recorded by the harness, so result correctness at 100 MB/1 GB is *not* validated by the benchmark (only at 10 MB by the vertical tests).

## 13. Performance Findings

- **Pruning works and scales with data size:** spatially coherent row groups skip 74–97 % of groups for a 0.6 % viewport; shuffled files skip 0 % (every row group's bbox spans the whole extent). Relative gain grows with size (clustered 0.6 % bbox: 4.1× at 10 MB, 4.9× at 100 MB, 25× at 1 GB; `return_rows`: 5.9× → 23× → 46×).
- **Uniform data prunes worse than clustered** (more row groups touch any window); the 25 % window prunes 54–75 %.
- **Attribute-only scan (`attr_filter_fullscan`) prunes nothing by design**; shuffled is still 1.7–2.1× slower at 1 GB (not explained; possibly different compression/decode cost — not investigated).
- **Unexpected:** DuckDB's profiler `total_bytes_read` is useless for absolute I/O (a full scan of two columns of a 98 MB file reports ~56 KB) and `operator_rows_scanned` equals rows × row groups. The harness therefore reports scan volume from statistics analysis, labelled as an upper bound, and DuckDB bytes only as a relative signal. **True bytes read were never measured** (no OS-level I/O counters were used).
- **Conversion/overheads:** WKB→GeoJSON for 20 k polygons ~0.6 s (debug); per-execution DuckDB setup ~100–180 ms (debug) is visible for sub-20 ms queries.
- Untested: remote object storage, cold cache, concurrency, > 1 GB, Overture-like data.

## 14. Architecture Deviations

1. Flat `crates/` instead of `engines/`, `formats/`, `gateway/ogc-*`, `sdk/`.
2. IR is a tree, not a DAG (RFC-001).
3. Execution reads Parquet inside DuckDB; the Arrow-native `GeoParquetSource::read` is not on the execution path. "Row groups skipped" is computed by GeoFabric from statistics, **not** reported by DuckDB; whether DuckDB prunes identically is inferred only from timing (consistent with it).
4. `rows_scanned` is a statistics upper bound, not a measurement.
5. UI "SQL / Ask" tab accepts logical IR / template parameters, **not SQL** (brief allowed "direct query or compile IR"; raw SQL deferred because of the trust model). No "Ask"/AI.
6. Planner has no cost model and only one real engine; cost-guard thresholds are placeholders.
7. Process: Codex was used for **one** task (geo-types); its output violated the spec (it modelled CRS and units as `String` wrappers and invented `Quantity`) and its sandbox had no network to run `cargo test`, so it was rejected and the crate was written by Claude. All later code was written by Claude. The lead/implementer split in the brief was therefore not followed beyond that attempt.
8. GeoJSON appears at the gateway/UI boundary only (allowed by Constitution 1); WKT literals appear inside IR (`Intersects`) as the boundary encoding for query geometry.
9. ADRs 001–006 and RFCs 001–003 are still `Proposed`; ADR-001..005 were not edited to include the evidence gathered here.
10. Buffer assumes metre-based projected CRSs from a 4-entry table; other EPSG codes are refused as "kind unknown".

## 15. Technical Debt

- **P0:** no per-query timeout/interrupt (a hostile or accidental expensive IR can hold a worker); no authentication if the gateway is bound beyond localhost.
- **P1:** result correctness not validated in the 100 MB/1 GB benchmarks; spatial-join test is vacuous; Buffer execution untested; planner reads Parquet footers (cheap, but done before engine admission — mitigated by `check_source_access`); no CI; real CRS database (PROJ) absent; `Transform` node missing.
- **P2:** DuckDB connection per query (spatial reload); only WKB sources; cost model; true I/O accounting; gateway/UI automated tests; target/ size (~38 GB currently, mostly stale debug artefacts).
- **P3:** DAG IR; UI accessibility audit; light theme not reviewed; table caps at 1 000 rows; basemap-less map.

## 16. Security Findings

- **Arbitrary SQL:** not accepted anywhere; SQL is generated from validated IR with quoted identifiers and escaped/numeric literals; WKT restricted to a safe charset; tests cover a quote-injection literal. The accepted-risk list: column names come from the file schema (quoted, but a hostile file could contain odd names).
- **File access:** runtime pre-check + engine canonicalised allow-list + DuckDB `allowed_directories` with external access disabled (defence in depth). Verified: out-of-root source → 403; `../` source id → 400. Symlinks inside the data directory are followed by `canonicalize` and then checked against the root (resolved target must be inside).
- **Network:** remote URIs refused; the spatial extension install is the only network access (adapter config `allow_extension_install`).
- **Memory / cost:** `memory_limit`, `max_rows` (100 001 at the gateway), request body 1 MiB, 2 concurrent queries; **no CPU/time limit** (P0).
- **Dependencies:** no `cargo audit` / `npm audit` beyond `npm install` reporting 0 vulnerabilities; the large C++ DuckDB build is a supply-chain surface; Python tooling is dev-only.
- **UI:** all file-derived strings are inserted with `textContent` (no `innerHTML`).

## 17. Open Questions for the Principal Architect

1. Is "absent GeoParquet CRS ⇒ Unknown (strict)" acceptable, or should CRS84 be accepted with an explicit warning?
2. Should `Intersects`' literal be WKT in IR, or a typed geometry (WKB + CRS)? WKT parsing for envelope extraction is a best-effort scanner today.
3. Should rows-scanned/row-groups-skipped be measured by the engine (contract change to RFC-002) rather than derived? Is a DuckDB patch/extension counter acceptable?
4. Bundled DuckDB costs ~38 GB of `target/` and 3–5 min clean builds — move to a system/dynamic DuckDB (ADR-006)?
5. Is the planner allowed to depend on `geo-source-geoparquet` (storage) for statistics, or should sources be exposed through a catalog trait?
6. Boundary format for map delivery: GeoJSON now; vector tiles (MVT/MLT) or GeoArrow → deck.gl next? Needs evidence (ADR-005).
7. Tree vs DAG IR before more node types are added.

## 18. Proposed Next Phase (not started)

| task | reason | dependency | complexity | architectural impact |
|---|---|---|---|---|
| Per-query timeout + interrupt | closes the P0 | adapter | S | `ExecutionContext` gains `timeout`; RFC-002 note |
| Validate counts in the benchmark harness against an oracle | remove the biggest evidence gap | harness | S | none |
| Projected-CRS dataset + Buffer/Transform node | exercise metric semantics | IR | M | adds node kind → RFC-001 revision |
| Engine-reported scan metrics (or I/O counters) | real bytes read | RFC-002 | M | contract change |
| Second real engine (e.g. WASM or PostGIS read-only) | proves replaceability | planner cost model | L | RFC-003 evolution |
| Real data (Overture subset) benchmark | needs approval — **not** downloaded | owner decision | M | none |
| CI + arch_check wiring | enforce the Constitution | none | S | none |

## 19. Files Changed (important)

`CONSTITUTION.md`, `AGENTS.md`, `docs/vision.md`, `rfcs/*`, `adr/*` (incl. new ADR-006), `tools/arch_check.py`, `crates/*` (all), `web/src/main.ts`, `web/src/style.css`, `benchmarks/gen_synthetic.py`, `benchmarks/measure_*.sh`, `benchmarks/results/*`, `Cargo.toml` (workspace, `[profile.dev] debug="line-tables-only"` — applied; the build log in ADR-006 predates it).

## 20. Self-Assessment (0–10)

| area | score | why |
|---|---|---|
| architecture | 7 | boundaries enforced and tested; but tree IR, one real engine, planner reads storage |
| correctness | 6 | oracle-checked on 10 MB only; join/buffer unverified; large-run counts not validated |
| performance | 6 | pruning effect measured, true I/O not; debug-only conversion numbers |
| testability | 6 | 44 tests, no coverage tool, no gateway/UI tests, data-dependent skips |
| maintainability | 7 | small crates, documented rules; some duplicated path checks |
| developer experience | 5 | 38 GB `target/`, 3–5 min clean builds, extension download, editor lag |
| UI | 5 | layout and plan view work; Inspector/Results not browser-verified; no basemap; sub-pixel features at low zoom |
| documentation | 7 | README demo, ADR-006 with measurements; ADR-001..005 not updated with evidence |
