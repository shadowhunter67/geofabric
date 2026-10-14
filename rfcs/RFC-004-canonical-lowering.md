# RFC-004 — Shared canonical lowering (research proposal, no code)

Status: Proposed (2026-10-14). **Research only**: nothing in this RFC is implemented, and no existing format or contract changes.

## Problem (architecture debt, P1)

The logical plan (PlanGraph, IR 0.2) is lowered to SQL **twice, by hand, in two languages**:

| | Rust | TypeScript |
|---|---|---|
| file | `crates/geo-engine-duckdb/src/sql.rs` (360 lines, all 10 operators) | `web/src/engine/sqlgen.ts` (172 lines, 7 operators) |
| runs in | the gateway (DuckDB, spatial extension) | the browser (DuckDB-Wasm, no spatial extension) |

They must agree on identifier quoting, literal escaping, NULL/comparison semantics, `bbox_filter` through the covering columns, the result column order and the geometry encoding. Today the only guard is the cross-engine digest tests (same plan, both engines, equal `result_digest`). That is a good oracle but a *late* one: a divergence is found by a failing digest, not prevented, and it only covers the plans somebody wrote a test for.

Evidence that the duplication already bites: the browser compiler had a defect that the server compiler never had (typed `GEOMETRY(crs)` columns cannot be streamed out of DuckDB-Wasm; the Rust side converts with `ST_AsWKB` at the scan, the TypeScript side did not). It was fixed on 2026-10-14 (`sqlgen.ts`, source CTE) after the runtime probe could not read the Overture fixture. The planner had been told for a week that "DuckDB-Wasm cannot read the file"; the file was fine, the lowering was wrong.

## Options

**A. One compiler, two hosts (compile the Rust lowering to WebAssembly).** `sql.rs` already is the reference; build it for `wasm32` and call it from the browser. One implementation, dialect differences as data (`spatial: bool`). Cost: a second wasm module in the page (size, load time), the `duckdb` crate and `geo-source-geoparquet` must be split from the lowering (it currently reads footers to decide a few things), and the browser then depends on a Rust→wasm toolchain in the build. Risk: the lowering is not pure today (it consults file metadata).

**B. A declarative lowering table consumed by both.** Describe each operator as a SQL template plus a dialect table in JSON (`Filter: "SELECT * FROM {in} WHERE {expr}"`, `BboxFilter: dialect.spatial ? ... : covering form`, `geometry_out: "ST_AsWKB({col})"`), and write a *tiny* interpreter in each language (~60 lines). The templates, the quoting rules and the geometry rule live in one reviewed file. Cost: two interpreters instead of two compilers; expressiveness is limited to what templates can say (joins with prefixing need care).

**C. Substrait (or another standard plan format) between the planner and engines.** Standard, engine-neutral, and DuckDB has a Substrait extension. Cost: it lowers *relational* algebra; spatial operators and CRS/unit semantics are exactly what GeoFabric's IR adds, so we would still need extensions on both sides, and DuckDB-Wasm does not ship the extension. Premature.

**D. Keep two compilers, make drift impossible to miss (conformance suite).** Golden SQL fixtures per operator and dialect checked by both languages, plus a differential test that runs every fixture plan on both engines and compares digests. Cost: tests only, no runtime change.

## Recommendation

1. **Now (cheap, no architecture change): D.** A shared fixture file `crates/geo-ir/tests/fixtures/lowering/*.json` (plan + expected canonical SQL per dialect) read by `cargo test` and `node --test`. The first fixtures should be exactly the cases that already diverged: the geometry conversion at the scan, covering-column `bbox_filter`, NULL handling in `Filter`.
2. **Next, if a third engine or a third client arrives: B.** With PostGIS (SQL again) or another client engine, the number of lowerings grows; a template table amortises best then.
3. **Do not do A or C now.** A couples the browser build to the Rust toolchain for 170 lines of logic; C does not remove the spatial extension problem.

## What would change this

Measure drift instead of fearing it: count lowering defects found by digest tests versus by users. If the count after D is zero for a release cycle, the duplication is tolerable; if a second defect of the `ST_AsWKB` kind appears, do B.

## Not decided here

The IR serialisation (0.2) does not change under any option. The planner does not learn SQL. ADR-008 stays Proposed.
