# REPORT — GeoFabric Multi-Engine Architecture Validation

For the Principal Architect. Date: 2026-10-13. One Windows 11 machine (28 logical cores, 32 GB RAM); browser = installed Chrome, headless, **on the same host as the gateway** (loopback network). Nothing here is verified on a second machine. Not production-ready.

## 1. Executive Summary

The phase goal — *the same logical plan can run in different places chosen from the workload* — is demonstrated **for two executors of one engine family**:

| claim to prove | result |
|---|---|
| IR can move from tree to DAG without changing behaviour | **yes**: PlanGraph (IR 0.2), 14 DAG tests, 0.1→0.2 migration of fixtures produced by the old code, all 84 pre-existing benchmark results byte-identical (hash) after the change |
| Planner coordinates ≥ 2 real engines | **yes**: DuckDB (server) and DuckDB-Wasm (browser); candidates, scores, hard limits, explanations |
| Runs on real data | **yes**: 1.45 GB Overture buildings subset (15.45 M rows), 7/7 queries match an independent pyarrow+shapely oracle |
| Browser/local execution is feasible | **yes, in a narrow zone**: it is faster than the server only for small files (≈ 10 MB) with aggregates / selective viewports (8 of 24 comparable cases); on 100 MB files it is 3–5× slower; ≥ 1 GB or Overture-like files are infeasible/unsafe in a tab |

Also done: dynamic (prebuilt) DuckDB for development with sha256 verification; cross-engine digest equality; UI shows engine, alternatives, reasons and fallback.

**Honest limits.** Both engines are DuckDB (different builds) — replaceability across *different* engine families is still unproven. The cost model was fitted on this machine with loopback networking; the network term is an assumption. Measured browser failure modes (tab crash, unreadable Parquet geometry type) are handled by recycling and fallback, not predicted. Multi-stage plans (splitting one graph across engines) are **not** implemented. The bundled (CI/release) DuckDB path was **not** re-run this round.

## 2. Repository State

Branch `main`; head {{COMMIT}}; previous approved commit `c865a41`. Milestone commits: prebuilt DuckDB workflow → PlanGraph + planner v1 → DuckDB-Wasm engine + cross-engine tests → calibration + selection tests + docs.
Tests: **Rust 98**, **TypeScript unit 17** (`node --test`), **browser e2e 9** (smoke 1, engines/UI 8; plus the manual crossover run) — all pass; `arch_check` passes (now with the browser descriptor crate).

## 3. PlanGraph / DAG

- Model: `Plan { ir_version "0.2", nodes: BTreeMap<NodeId, PlanNode>, root, hints }`; nodes reference inputs by `NodeId`. `GraphBuilder` content-addresses nodes (id = `n_` + 20 hex of the node's JSON hash, inputs already by id), so equal sub-plans collapse. `plan_id()` hashes *content* (canonical hashes of the root), independent of how ids are spelled.
- Validation (on the graph, once per node, topological order): root exists, no missing reference, **no cycle**, **no orphan**, plus all existing CRS/unit/schema rules (verdicts of the 12 pre-existing IR tests unchanged). Output schemas are derived once per node.
- The tree `Node` type remains a builder (`Plan::new(Node)` shares duplicates) and `Plan::to_tree()` expands back.
- Migration: `Plan::from_json` accepts 0.1 and returns a 0.2 graph (`migrate_v0_1`, deterministic); other versions are rejected. Fixtures were generated **with the 0.1 implementation before the refactor** (5 files incl. a self join).
- Consumers on the graph: DuckDB adapter (one `NOT MATERIALIZED` CTE per node; a shared source compiles once), scan analysis (shared source = union of access paths), planner, runtime, gateway, TS executor.
- Tests: valid linear; branching with shared source **and** shared intermediate; builder collapse; tree→graph sharing; cycle (incl. self-loop) rejected; missing node and missing root rejected; orphan detected; topological order (inputs first, deterministic); serialization roundtrip/versioning; plan id independent of id spelling; migration: old fixtures load, deterministic, tree-equivalent, same schema/ops, validation-equivalent, hints preserved; shared-source DAG executes with an **exact** pair result and one CTE; tree and DAG forms of one plan give identical digests.
- Limitation: DuckDB inlines `NOT MATERIALIZED` CTEs, so a shared node is *compiled* once but still *evaluated* per consumer.

## 4. Planner v1

- Inputs: PlanGraph, source metadata (footers, via the access guard), `[EngineDescriptor]` (capabilities incl. `kind`, hard limits, `bbox_filter_requires_covering`), `PlannerConfig`. Output: `PhysicalPlan` (selected engine, placement, `stages` (one), `candidates[]`, scans, estimate, decision log, engine steps).
- Contract change (RFC-002 revised, flagged for review): `EngineDescriptor` (what the planner needs) vs `ExecutionEngine: EngineDescriptor` (in-process execution). The browser engine is a descriptor in Rust (`geo-engine-browser`) executed by a TypeScript implementation; the runtime returns `Dispatch::Client`.
- **Feasibility is a hard filter** (never traded against score): operations supported; source format; browser enabled; estimated input (kept data **+ covering columns of the whole file** for the browser) within the engine limit and `browser_max_input_bytes` (default 50 MB); result rows within limit; bbox covering column present when the engine needs it.
- **Score (ms)**, all parameters in `PlannerConfig.cost`, fitted by non-negative least squares on **relative** error (a first fit on absolute error mis-ranked every small case):
  `server = 83 + 0.199·input_MB + 17.8 µs·rows`; `browser = 19 + 5.38·covering_MB + 0.58·input_MB + 96 µs·rows`; plus `bytes/network` (default 1000 MB/s = assumption).
- Result rows are estimated by a uniform-density selectivity heuristic per kept row group (measured: median 6× too high, range 1.2×–113× on non-truncated cases; clustered data breaks uniformity) — an estimate, not a bound.
- Explainability: every candidate carries feasible/score/cost breakdown/reasons/rejections; the log records validation, pruning, "covering inferred" warnings, candidates, hints, ties, the selection and its alternatives. The UI renders it.

## 5. DuckDB

- **Dynamic/prebuilt validated.** `tools/duckdb_prebuilt.py` derives v1.5.6 from `Cargo.lock`, downloads `libduckdb-windows-amd64.zip`, verifies sha256 against **GitHub's published digest and the committed lock file** (`44cf5958…fe96a`), rejects unsafe archive paths. `cargo test --workspace --no-default-features` passes (clean build + all tests ≈ 61 s vs ≈ 10 min bundled); features forwarded runtime/gateway/bench; `target/` small. ADR-006 updated: dev default = prebuilt, CI/release = bundled-pinned **(bundled not re-run this round)**.
- **Regression (D).** All Rust/gateway tests pass; benchmarks 10 MB / 100 MB / 1 GB: **84/84 runs `pass`** against the independent oracles, and every result **count and hash is identical** to the previous (hardening, bundled) round. Median engine time ratio new/old ≈ 0.6 (0.47–1.12 per query) — attributable to the official optimized library and a quiet machine, not to the DAG; worst slowdown +12 % (noise range). 12 join-type rows show different row-group accounting because a shared source is now counted once (accounting change, not an execution change).

## 6. Overture (real data)

- **Dataset.** Overture Maps buildings, release 2026-09-23.1, bbox 105–108 °E, 8.5–12 °N (southern Vietnam incl. Ho Chi Minh City), fetched with DuckDB/httpfs (`benchmarks/overture_fetch.py`) into one local GeoParquet; **1 451 854 492 B, 15 453 032 rows, 154 row groups** (mean 100 344 rows, min 2 632), ZSTD, 45 leaf columns, written by DuckDB 1.5.6. Not committed (ODbL/attribution; see `.fetch.json`).
- **Metadata findings** (`benchmarks/inspect_parquet.py`, `results/overture-inspect.json`): `geo` v1.0.0, primary `geometry` WKB, types **Polygon + MultiPolygon**, **`crs` key absent → OGC:CRS84** (the new CRS semantics were exercised for real), **no `covering` declared** but a physical `bbox` struct column (field order xmin, xmax, ymin, ymax) with statistics in **all 154 row groups**; attributes are sparse (`class` 99.9 % NULL, `height` 99.996 % NULL) so attribute benchmarks use `version`; nested columns `names` (struct with map and list), `sources` (list of structs).
- **What changed because of it.** The reader now infers the covering by convention (struct `bbox{xmin,ymin,xmax,ymax}` doubles), marks it `declared: false`, and the planner logs a warning; feature templates no longer project nested columns; the digest learned int32/bool/float cells.
- **Benchmark (release, prebuilt DuckDB; all `pass` vs the oracle — pyarrow for bbox/attributes, shapely/GEOS for geometry predicates):**

| query | result count | result hash (12) | oracle | row groups skipped (est.) / total (obs.) | bytes <= (est.) | bytes read | median ms (obs.) | DuckDB peak buffer MB (obs.) |
|---|---|---|---|---|---|---|---|---|
| ov_bbox_count_small | 812,006 | 6e433193fc37 | pass | 140/154 | 131 MB | unknown | 47 | 29 |
| ov_bbox_count_large | 12,648,395 | 6099a19e218a | pass | 9/154 | 1371 MB | unknown | 206 | 37 |
| ov_attr_filter_fullscan | 8,599,896 | 0b8161df6651 | pass | 0/154 | 1447 MB | unknown | 41 | 5 |
| ov_intersects_polygon | 284,071 | 06c475ea8076 | pass | 140/154 | 131 MB | unknown | 236 | 180 |
| ov_aggregate_version_small | 10 | 60bb979d0b28 | pass | 140/154 | 131 MB | unknown | 41 | 28 |
| ov_spatial_join_small | 145 | be47addbcb8f | pass | 150/154 | 38 MB | unknown | 93 | 161 |
| ov_return_rows_small(id,version) | 28,169 | 041309ba7902 | pass | 150/154 | 38 MB | unknown | 50 | 16 |

- **Questions asked in the brief.**
  - *Does synthetic represent real data?* Partly. Pruning transfers (real 20 km window: 140 of 154 row groups skipped, 9 of 154 for a 25 % window); correctness paths transfer. It does **not** represent: dense urban clusters (812 006 buildings in the 20 km window), nested/null-heavy schema, CRS84 with no covering metadata, Polygon+MultiPolygon, 100 k-row groups with overlapping bbox ranges, and browser behaviour (below).
  - *Is row-group pruning still effective?* Yes for DuckDB on the server (estimated from statistics; true I/O unknown). **No for DuckDB-Wasm**: it does not prune through struct-field statistics in the build used, so it reads all bbox column chunks of the file (cost modelled via `covering_MB`).
  - *Is GeoParquet metadata sufficient?* Not always: no `covering`, no CRS key (spec default applies) — handled by inference/semantics, flagged.
  - *Do nested schemas cause problems?* Yes in the browser: DuckDB-Wasm failed to read the Parquet native GEOMETRY column of this file (`TransactionContext::ActiveTransaction called without active transaction`, the same family of error as the Rust binding's Arrow export of native GEOMETRY); aggregates over non-geometry columns worked.
  - *Memory behaviour?* DuckDB's buffer peak ≤ 180 MB on all Overture queries (intersects 180 MB, join 161 MB). The browser tab **crashed** at ≈ 245 MB (kept 112 MB + covering 139 MB) and after repeated large files in one instance.
  - *Planner estimates vs reality?* Input bytes: upper bound by construction (true I/O unknown). Result rows: median 6× over-estimate (1.2–113×). Time: server median absolute error 38 ms, browser 188 ms (repeat run).

## 7. WASM / Browser Executor

- Engine: DuckDB-Wasm (`@duckdb/duckdb-wasm` 1.33.1-dev57) in a Web Worker, behind the contract (`web/src/engine/`: `contract.ts`, `sqlgen.ts` IR→SQL for the supported subset, `digest.ts`, `wkb.ts`, `wasm.ts`). The UI never calls it directly: planner decides, gateway returns PlanGraph + authorized URLs, the browser fetches Parquet byte ranges (`/api/files/{id}`, HTTP range, authorization-guarded) and executes.
- Supported: Source, Filter, BboxFilter (via covering columns only — no spatial extension), Aggregate, Project, Limit, Sort. **Not** supported (planner never offers them): Intersects, Buffer, SpatialJoin. Safety net: the TS compiler throws `unsupported` for anything else; identifiers/literals quoted; cycle/missing-node checks; limits `maxRows`, timeout (`cancelSent()`), AbortSignal.
- Memory safety: planner hard guards (`browser_max_input_bytes`, `browser_max_result_rows`, engine cap 256 MB, input counted as kept data + whole-file covering bytes) and instance recycling after 192 MB cumulative input.
- Limitations: no CPU/real I/O metrics (`bytes_read` unknown); cold start ≈ 1.3–1.5 s; the failure modes above; timeout/cancel exists but was **not** exercised against a long-running browser query (no e2e).
- Tests: 17 TS unit tests (sqlgen: covering predicate, shared source once, escaping/injection, unsupported rejection, structural errors; digest vs Python hashlib; WKB decoder incl. hostile counts), 8 e2e (below).

## 8. Cross-engine Correctness

Same PlanGraph, canonical digest (rows sorted, cells canonical, SHA-256; identical definition in Rust, TS and the Python oracles):

| query (clustered_10mb_spatial, viewport 104-105E, 12-13N) | rows | DuckDB (server) hash | DuckDB-Wasm (browser) hash | match |
|---|---|---|---|---|
| features in viewport (bbox, limit 100000) | 33 | 84159c1b8a962d16… (count 33) | 84159c1b8a962d16… (count 33) | **yes** |
| count by category in viewport (aggregate) | 8 | 333ce7595d2ad459… (count 8) | 333ce7595d2ad459… (count 8) | **yes** |
| features, category = school (attribute filter) | 6 | 2b00384a7bb5a588… (count 6) | 2b00384a7bb5a588… (count 6) | **yes** |

Further: in the crossover runs every deterministic (`count_by`) comparison matched (8/8) and row counts matched for all feature queries that ran on both engines (16/16); **0 mismatches** in any run. Geometry bytes (WKB) are part of the digest for the features query. Floats are digested by shortest round-trip spelling (documented limit: magnitudes outside [1e-6, 1e21) differ between Rust and JS).

## 9. Engine Selection

Planner choice under the **default configuration** vs the faster engine measured in the clean crossover run (run 5; model fitted on runs 1+4 of the same grid, so this is a repeatability check, **not** a held-out validation). 48 grid cases; 24 ran on both engines (the rest: browser infeasible under the planner's limits or skipped as unsafe).

| dataset | window (% extent) | query | planner selected (default config) | scores s / b (ms) | measured s / b (ms) | actual fastest | planner correct? |
|---|---|---|---|---|---|---|---|
| clustered_10mb | 0.2% | count_by | browser | 84 / 53 | 75 / 49 | browser | yes |
| clustered_10mb | 0.2% | features (limit 2000) | browser | 87 / 70 | 79 / 56 | browser | yes |
| clustered_10mb | 0.2% | features (limit 50000) | browser | 87 / 70 | 80 / 35 | browser | yes |
| clustered_10mb | 1% | count_by | browser | 84 / 54 | 74 / 71 | browser | yes |
| clustered_10mb | 1% | features (limit 2000) | server | 105 / 166 | 82 / 41 | browser | NO |
| clustered_10mb | 1% | features (limit 50000) | server | 105 / 166 | 87 / 55 | browser | NO |
| clustered_10mb | 5% | count_by | browser | 84 / 55 | 139 / 79 | browser | yes |
| clustered_10mb | 5% | features (limit 2000) | server | 120 / 242 | 192 / 410 | server | yes |
| clustered_10mb | 5% | features (limit 50000) | server | 183 / 578 | 248 / 781 | server | yes |
| clustered_10mb | 25% | count_by | browser | 85 / 58 | 128 / 75 | browser | yes |
| clustered_10mb | 25% | features (limit 2000) | server | 120 / 245 | 172 / 413 | server | yes |
| clustered_10mb | 25% | features (limit 50000) | server | 547 / 2511 | 651 / 3369 | server | yes |
| clustered_100mb | 0.2% | count_by | server | 85 / 311 | 132 / 603 | server | yes |
| clustered_100mb | 0.2% | features (limit 2000) | server | 106 / 423 | 132 / 687 | server | yes |
| clustered_100mb | 0.2% | features (limit 50000) | server | 106 / 423 | 147 / 629 | server | yes |
| clustered_100mb | 1% | count_by | server | 85 / 316 | 138 / 556 | server | yes |
| clustered_100mb | 1% | features (limit 2000) | server | 121 / 503 | 159 / 711 | server | yes |
| clustered_100mb | 1% | features (limit 50000) | server | 199 / 921 | 162 / 573 | server | yes |
| clustered_100mb | 5% | count_by | server | 86 / 323 | 135 / 502 | server | yes |
| clustered_100mb | 5% | features (limit 2000) | server | 121 / 510 | 209 / 715 | server | yes |
| clustered_100mb | 5% | features (limit 50000) | server | 990 / 5118 | 1251 / 4118 | server | yes |
| clustered_100mb | 25% | count_by | server | 91 / 359 | 83 / 322 | server | yes |
| clustered_100mb | 25% | features (limit 2000) | server | 126 / 546 | 110 / 350 | server | yes |
| clustered_100mb | 25% | features (limit 50000) | server | 994 / 5154 | 803 / 4923 | server | yes |
| clustered_1gb | 0.2% | count_by | server | 88 / infeasible | 86 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 0.2% | features (limit 2000) | server | 123 / infeasible | 100 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 0.2% | features (limit 50000) | server | 234 / infeasible | 115 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 1% | count_by | server | 91 / infeasible | 94 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 1% | features (limit 2000) | server | 126 / infeasible | 122 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 1% | features (limit 50000) | server | 769 / infeasible | 213 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 5% | count_by | server | 102 / infeasible | 110 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 5% | features (limit 2000) | server | 138 / infeasible | 121 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 5% | features (limit 50000) | server | 1006 / infeasible | 825 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 25% | count_by | server | 138 / infeasible | 156 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 25% | features (limit 2000) | server | 173 / infeasible | 136 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 25% | features (limit 50000) | server | 1041 / infeasible | 811 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 0.2% | count_by | server | 105 / 1038 | 125 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 0.2% | features (limit 2000) | server | 141 / 1225 | 189 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 0.2% | features (limit 50000) | server | 1009 / 5833 | 1357 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 1% | count_by | server | 134 / infeasible | 139 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 1% | features (limit 2000) | server | 169 / infeasible | 186 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 1% | features (limit 50000) | server | 1038 / infeasible | 1458 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 5% | count_by | server | 186 / infeasible | 180 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 5% | features (limit 2000) | server | 222 / infeasible | 191 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 5% | features (limit 50000) | server | 1090 / infeasible | 1415 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 25% | count_by | server | 272 / infeasible | 244 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 25% | features (limit 2000) | server | 308 / infeasible | 185 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 25% | features (limit 50000) | server | 1176 / infeasible | 1371 / not run | server (browser not run) | yes (browser infeasible under the default guard) |

Winner agreement **22/24**; browser faster in 8 cases (predicted 6). Both misses: 10 MB file, 1 % window, features — the row estimate (1222) is 18× the actual (66) so the browser looked too expensive. Selection matrix tests with the real engines and files (`selection.rs`, 10 tests): small file + selective viewport → browser; 100 MB → server on score; 1 GB and Overture → server by memory guard; unsupported op (intersects, spatial join) → server with reason; browser memory limit and result-row limit are hard constraints; browser disabled → server; hints honored only when feasible; large result → server; every candidate explained.

## 10. Crossover Analysis

Clean run (fresh tab per dataset, no recycling), server vs browser total time as seen by the page (browser = plan request + file ranges + DuckDB-Wasm + conversion; server = request + execution + JSON), medians of 3:

| dataset | query | server ms (min-max) | browser ms (min-max) | browser faster in |
|---|---|---|---|---|
| clustered_10mb | count_by | 74-139 | 49-79 | 4/4 |
| clustered_10mb | features | 79-651 | 35-3369 | 4/8 |
| clustered_100mb | count_by | 83-138 | 322-603 | 0/4 |
| clustered_100mb | features | 110-1251 | 350-4923 | 0/8 |
| clustered_1gb | count_by | 86-156 | not run / not placed | 0/0 |
| clustered_1gb | features | 100-825 | not run / not placed | 0/0 |
| overture_buildings_vn_south | count_by | 125-244 | 4312-4312 | 0/1 |
| overture_buildings_vn_south | features | 185-1458 | not run / not placed | 0/0 |

What the evidence supports (and does not):
- **Browser wins**: file ≈ 10 MB (≈ 26 row groups, ≈ 4.5 MB of covering columns) with an aggregate or a selective viewport (≤ a few hundred rows): 35–79 ms vs 74–139 ms.
- **Server wins**: any file ≥ 100 MB (browser 320–700 ms for a tiny result because it reads all covering columns: ≈ 46 MB), and **any query returning many rows** — browser ≈ 96 µs/row vs server ≈ 18 µs/row (10 MB file, 19 475 rows: 3.4 s vs 0.65 s).
- **Crossover point** on this machine: file size around 10 MB, not a function of the *queried* window alone. Between 10 MB and 100 MB there are no measurements, so the exact threshold is **unknown**; the default guard (50 MB) is a conservative choice, not a measured crossover.
- Network: measured on loopback, i.e. **no real network cost** — over a real link the browser (which pulls data) loses earlier, the server (which ships results) later. The model's `network_mb_per_s` encodes that assumption.
- Exceptions: first browser query pays ≈ 1.3–1.5 s instance start; Overture-like files crash/fail in the browser; a single instance accumulating several large files crashed (hence recycling).

## 11. UI

Execution Plan tab (see `docs/screenshot-ui.png` — server selected — and `docs/screenshot-ui-browser-engine.png` — browser selected, score 66.7 ms vs 86.7 ms): a **planner panel** shows the selected engine (`Server / DuckDB` or `Local Browser / DuckDB-Wasm`) with a local/server badge, score, estimated input, estimated transfer (to the browser or of the result), the reasons, each **alternative engine** with its score or rejection reason, and — when the browser failed — a visible **fallback** notice. Query tab: engine selector (auto / server / browser) and "allow browser execution". Metric cards keep provenance badges (Observed/Estimated/Derived/Unavailable).
Smoke/e2e results: **PASS** — app + map + sources; Execution Plan with badges; Results; Inspector (click on a feature); browser engine selected through the UI shows `data-engine="duckdb-wasm"`, "Local (browser)", the alternative `duckdb`, estimated transfer, rows in Results; auto mode shows the server engine, the browser alternative and the estimated input; engine failure (injected) → re-run on the server with the fallback shown; unsupported operation and "browser disabled" never reach the browser.

## 12. Benchmarks (summary)

Synthetic 10 MB / 100 MB / 1 GB: 84 runs, all `pass` (count+hash vs oracle), see `benchmarks/results/bench-p3-*.md`. Real Overture: 7/7 `pass` (table in §6). Crossover: `crossover-run{1,4,5}.json` (run 5 clean), planner comparison `planner-vs-measured-*.json`. Total oracle-validated engine runs this phase: 91. `bytes read` stays **unknown** everywhere.

## 13. Architecture Deviations

1. Planner contract changed (descriptor/executor split, `Capabilities.kind/limits`, `Estimate` provenance) — requires your review.
2. Flat `crates/` layout (unchanged); WASM engine is TypeScript, not a Rust-on-wasm crate (the Rust side is only a descriptor).
3. Both engines are DuckDB; planner "multi-engine" plans are single-stage only.
4. Browser-side SQL generation is a second implementation (TS) of the IR→SQL mapping; consistency is guaranteed only by the cross-engine digest tests, not by shared code.
5. Covering inferred by naming convention when not declared (flagged, but a heuristic).
6. Cost model parameters fitted on one machine/loopback; `network_mb_per_s` is an assumption.
7. Process: Codex was used for **one** small task (the TypeScript WKB decoder; reviewed, verified with `node --test`, renamed export `wkbToGeoJSON`, BOM removed); everything else was written by Claude. The Codex sandbox could not run `node --test` (spawn EPERM) — tests were re-run here.
8. Overture subset lacks `covering`/crs metadata because it was re-written by DuckDB COPY; a pristine release file may differ.

## 14. Remaining Debt

- **P0:** none new from this phase. (Standing: no authentication — required before any non-localhost exposure.)
- **P1:** engines of a different family (PostGIS / different engine) to prove replaceability; multi-stage plans; browser capability probe before execution (Parquet GEOMETRY type, memory) instead of runtime fallback; real-network measurements; bundled/CI path re-verification; pruning for DuckDB-Wasm (a newer build or a different covering layout may fix it); cost model validated on a held-out grid/second machine; actual I/O measurement.
- **P2:** better result-row estimator (histograms); timeout/cancel e2e for the browser engine; crossover between 10 MB and 100 MB; remote object storage; shared-node materialisation knob.
- **P3:** UI polish (planner panel screenshot refresh, light theme), per-engine cost model versioning.

## 15. Next Phase Recommendation

**REWORK PLANNER** (narrowly): add a *capability probe + learned estimates* step — probe the browser engine for the file (can it read the geometry type, how many covering bytes, footer size), replace the uniform-density row estimator, and validate the model on a held-out grid and on a real network — **before** adding PostGIS. Reason: the measured weak points are not missing engines but wrong/optimistic predictions (rows 6× over, browser crash/unreadable-type found only at runtime). Then ADD POSTGIS as the first *different-family* engine (the real replaceability test). Do not start AI or remote storage first.

## 16. Files Changed (important)

`crates/geo-ir/src/{graph,schema,validate,node,lib}.rs`, `crates/geo-ir/tests/{dag,ir}.rs` + `tests/fixtures/v0_1/*`, `crates/geo-executor/src/lib.rs` (descriptor/executor, digest, provenance), `crates/geo-source-geoparquet/src/{analysis,lib}.rs`, `crates/geo-planner/src/lib.rs`, `crates/geo-engine-browser/` (new), `crates/geo-engine-duckdb/src/{lib,sql}.rs`, `crates/geo-runtime/{src/lib.rs,tests/{selection,join,timeout}.rs}`, `crates/geo-gateway/src/{lib,main}.rs`, `crates/geo-bench/src/main.rs`, `web/src/engine/*`, `web/src/main.ts`, `web/e2e/{engines,crossover,smoke}.spec.ts`, `web/test/*`, `benchmarks/{oracle_overture,overture_fetch,inspect_parquet,planner_vs_measured}.py`, `tools/duckdb_prebuilt.py` + lock, `rfcs/RFC-001..003`, `adr/ADR-003,004,006,007,008`, `docs/pipeline.md`, `README.md`, `CLAUDE.md`.
