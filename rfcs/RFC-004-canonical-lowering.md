# RFC-004 — Canonical lowering: one definition of what a plan means, three dialects

Status: **Accepted and implemented (2026-10-22)** for the predicate/sort/bbox/aggregate/semi-join/row-cap semantics and for the browser dialect; **request → PlanGraph unified (2026-10-23)** in the `geo-plan` crate (see "Request → plan" below). Remaining debt is listed at the end. No IR or wire-format change (IR stays 0.3).

## Problem (architecture debt that already cost a correctness bug)

The logical plan (PlanGraph) was lowered to SQL by **three hand-written compilers**: `geo-engine-duckdb/src/sql.rs` (server), `geo-engine-postgis/src/lower.rs` (PostgreSQL) and `web/src/engine/sqlgen.ts` (DuckDB-Wasm; with a fourth near-copy of the server one in Rust, `geo-engine-browser`, kept as its reference). Each re-decided the same semantics: the predicate tree, literal rules, what `BboxFilter` means, sort order and NULL placement, aggregate calls, the semi-join form, the row cap. The first live PostGIS run proved the risk: `BboxFilter` had drifted (geometry-envelope overlap instead of the covering-box overlap every other engine uses: 41,150 vs 41,165 rows). The duplication was only guarded by tests written per plan, i.e. *late* and *partial*.

## Decision

**A pure crate, `geo-lowering`, defines every semantic decision once; an engine adds only its dialect.** (Option A below for the browser: the same Rust code, compiled to WebAssembly.)

```
PlanGraph
   │  capability validation (planner; unsupported operations never reach a lowering)
   ▼
geo-lowering  (pure: no engine, no I/O, no file format; builds for wasm32)
   │  predicate tree · literal rules · BboxFilter = covering-box overlap · ORDER BY (NULLS LAST, byte-order text) ·
   │  aggregate table · SpatialSemiJoin = EXISTS, metres never degrees · Transform CRS names · row cap (+1)
   ├── DuckDB server renderer   geo-engine-duckdb  (relational skeleton: authorization, covering discovery, ST_* spatial functions)
   ├── PostGIS renderer         geo-engine-postgis (bind parameters, catalog, hidden covering columns, ST_AsBinary 'NDR', COLLATE "C")
   └── Browser renderer         geo-lowering::browser → WebAssembly → web/src/engine/lowering.ts  (no spatial extension; ST_AsWKB at the scan)
```

### What is canonical (in `geo-lowering`, tested once)

| Decision | Definition |
|---|---|
| Predicate tree | `And`/`Or`/`Not`, `In`, `IsNull`, `Compare`, `HashBucket`; empty `IN`/`AND`/`OR` and out-of-range `HashBucket` are refused for every engine (`predicate`, `PredicateDialect`) |
| Literals | finite numbers only, one portable spelling (`105.0`, `13.25`), magnitudes outside `[1e-9, 1e15]` refused (exponent forms differ between engines), integers beyond 2^53-1 refused where JavaScript is involved (`format_float`, `inline_literal`) |
| `BboxFilter` | **the file's covering box overlaps the window**, closed intervals, four comparisons (`bbox_covering_overlap`). The geometry envelope is never the decisive predicate when a covering box exists |
| Sort | explicit `ASC`/`DESC`, **`NULLS LAST` in both directions**, text compared byte-wise (DuckDB natively; PostgreSQL asks for `COLLATE "C"`); `count_by` ties are broken by the group key in the plan itself (`order_by`) |
| Aggregates | one function table; only `COUNT` may omit its column (`aggregate_call`) |
| `SpatialSemiJoin` | return a left row once if any right row satisfies the predicate: `EXISTS` (`semi_join_exists`); a distance is metres in the plane of a local metric CRS, an angular distance is refused (`within_distance_meters`) |
| `Transform` | only known CRSs; never guessed (`proj_name`); PostGIS maps them through its SRID table |
| Row cap | one extra row is requested so "exactly the cap" differs from "over the cap" (`row_cap`) |

### What stays in an engine (dialect and resources)

SQL function names and spelling of leaves (DuckDB inline literals vs PostgreSQL `$n::type` bind parameters and its refusal of a comparison that would need a type coercion), where geometry becomes WKB, authorization and covering discovery (DuckDB reads Parquet footers), the catalog and clipped-copy guards (PostGIS), CTE style, capability refusals.

## Options considered

**A. One compiler, two hosts (Rust compiled to WebAssembly) — CHOSEN for the browser.** One implementation; the page calls it. Cost: a 346 KB wasm (102 KB gzipped) and `rustup target add wasm32-unknown-unknown` in the web build. Measured: acceptable next to the 8 MB DuckDB-Wasm.
**B. The gateway ships the browser SQL.** Rejected as the sole route: the *static* deployment has no gateway; the page itself builds the plan there.
**C. Substrait.** Relational algebra only; no spatial/CRS/unit semantics; DuckDB-Wasm has no extension for it. Premature.
**D. Schema + code generation for both languages.** Adds a generator and a schema language to remove ~250 lines.
**E. Two compilers + golden fixtures.** Detects drift, does not prevent it. Kept as *additional* guard (below), not as the mechanism.

## Migration (done in this order; each stage kept its predecessor's tests green)

1. **Canonical layer + unit tests** (`geo-lowering`, 10 tests: spelling, refusals, bbox, sort, aggregates, semi join, CRS names, row cap).
2. **DuckDB server** moved onto it (`sql.rs`: its private `lit`/`expr`/`proj_name`/aggregate/sort code deleted).
3. **PostGIS** moved onto it (predicate tree through `PredicateDialect`, bbox exact part, sort, semi join, aggregate calls, row cap, literal rules). Verified live afterwards: 17/17 plans identical to DuckDB on the Demo B window with the full 3.85 M-building data, 16/16 on a 535 k-building window, Demo B ids equal to the independent oracle by SHA-256.
4. **Browser**: `lower_browser` moved into `geo-lowering::browser`, exported to WebAssembly (`gf_alloc`/`gf_free`/`gf_lower_browser`, JSON in/out, no wasm-bindgen), loaded by `web/src/engine/lowering.ts`. **`sqlgen.ts` (the TypeScript compiler) was deleted.** `geo-engine-browser::lower` remains as a thin compatibility wrapper for the planner crate and the golden fixtures.
5. **Guards**: `tools/arch_check.py` now requires every engine adapter to depend on `geo-lowering`, forbids engine crates from spelling `NULLS LAST`, `COUNT(*)` or `EXISTS (SELECT 1` themselves, forbids SQL fragments and `sqlgen.ts` in `web/src`, and keeps `geo-lowering` pure (no planner/executor/runtime/source/engine/UI dependency).

## Test strategy

* `geo-lowering` unit tests: each canonical decision, once.
* `crates/geo-runtime/tests/canonical_lowering.rs` (8 tests, offline): the SAME plan is rendered by DuckDB, PostGIS and the browser lowering and compared with the canonical text: covering-box `BboxFilter` (and "not the geometry envelope" on PostGIS), `NULLS LAST` + `COLLATE "C"` only for PostgreSQL text keys, identical predicate trees, `EXISTS` + `ST_DWithin` in metres + explicit transform CRSs, the browser's refusal of spatial operations, the angular-distance refusal, render-thinning (DuckDB family only), and the refusals every engine must share (empty `IN`, empty `AND`, sort without keys, `SUM` without a column).
* Golden fixtures (`geo-engine-browser/tests/fixtures/lowering/*.json`) are read by the Rust reference test AND by `web/test/lowering.test.ts`, which loads the real wasm and requires the same bytes: the web glue cannot change the SQL.
* Live (needs PostgreSQL, reported separately): the 17-plan cross-engine harness, Demo B against the oracle, the 8 live driver tests.
* Browser: static-mode and full-stack Playwright on Chrome and Edge.

## Compatibility

No IR change. DuckDB and browser SQL differ from before only in (a) explicit `ASC NULLS LAST` / `DESC NULLS LAST` (the previous implicit behaviour, now stated) and (b) the refusal of literals outside `[1e-9, 1e15]` or non-finite. PostGIS SQL is unchanged except the same literal rule. The fixtures were regenerated for (a).

## Request → plan (2026-10-23)

`User request → canonical request model → ONE PlanGraph builder → PlanGraph → geo-lowering → DuckDB | PostGIS | Browser`. The builder is the pure Rust crate `geo-plan` (`PlanRequest`, `build_plan`, structured `PlanError` codes `INVALID_FIELD`, `INVALID_FILTER`, `INVALID_BBOX`, `INVALID_CRS`, `UNKNOWN_DATASET`, `UNSUPPORTED_REQUEST`...). The gateway links it natively; the static page calls it through the lowering wasm module that is already loaded (`gf_build_plan`, JSON in/out, the plan returned as the exact `plan_json` string). `web/src/static/adapter.ts` only forwards the request, attaches the engine hint (a planner input, never part of the plan nodes) and reads the capability verdict. The request model carries no SQL and no engine detail; it is validated before any node is built; a request the browser cannot run (transform, spatial semi join, intersects...) is built exactly and refused by the capability layer (`missing_ops` against `SUPPORTED_OPS`), never approximated or silently dropped. No IR change (0.3); the request carries its own `schema_version`. Evidence: 40 golden request fixtures (`crates/geo-plan/fixtures/requests/`) whose native and wasm plans are byte-identical; the previous gateway's plans for 19 requests are reproduced byte for byte (3 refusals deliberately became structured `INVALID_FIELD`); `tools/arch_check.py` forbids plan-node construction in the gateway and in the web code.

## Remaining debt (honest)

* **Relational skeletons** (CTE chains, covering propagation through Project/Limit/Sort) are still per engine; only the *decisions* are shared. A true shared intermediate relational tree would remove them too; it was not needed to stop semantic drift and would be a larger rewrite.
* **Intersects/Buffer/SpatialJoin** keep engine-specific code (spatial functions), covered by the cross-engine tests but not by a shared definition beyond `semi_join_exists`.
* The wasm needs the Rust toolchain in every web build (documented in CONTRIBUTING; CI installs the target).
