# ADR-006 — DuckDB integration strategy

Status: Proposed (evidence below is from one Windows machine: 28 logical cores, 32 GB RAM)

## Decision (tentative)

Use the Rust `duckdb` crate with the `bundled` feature (DuckDB 1.5.6 via `libduckdb-sys 1.10506`) plus the `parquet` feature, behind the `ExecutionEngine` trait (RFC-002). The `spatial` extension is loaded at runtime (`INSTALL`/`LOAD`). Python DuckDB is used only as tooling (`benchmarks/.venv`: dataset generation, cross-checks) and is **not** on the execution path.

## Measurements (bundled build)

| item | value |
|---|---|
| clean **debug** build of `geo-engine-duckdb` (+ arrow 58 deps), `CARGO_BUILD_JOBS=8` | 169 s |
| clean **release** build of `geo-bench` (whole workspace + DuckDB), jobs=8 | 298 s |
| clean debug build of `geo-gateway` (axum/tokio added), jobs=3 | 299 s |
| incremental debug rebuild after touching `geo-planner` or `geo-types` (jobs=3) | ~10 s |
| free-RAM floor during clean build (jobs=8) | ~10.2 GB of 13.3 GB free at start → ≈3 GB peak (sampled every 10 s, so an under-estimate) |
| `release/geo-bench.exe` size | 29.4 MB (statically contains DuckDB + Parquet) |
| `target/` after debug + release builds | **~29 GB** (debug 26 GB, release 2.4 GB) |

## Findings / developer friction

1. Bundled build is acceptable in time (3–5 min once, ~10 s incremental) but **very large on disk**: 26 GB for the debug profile. It also made VSCode laggy while building and while rust-analyzer indexed `target/`. Mitigation applied: `[profile.dev] debug = "line-tables-only"` in the workspace `Cargo.toml` (takes effect on the next clean build); exclude `target/` from the editor file watcher/search; build with `CARGO_BUILD_JOBS=3` while working.
2. **Debug-profile DuckDB is compiled without optimization** (cc uses the cargo profile's opt level). Benchmarks must use `--release`.
3. The spatial extension is **not bundled**: first `INSTALL spatial` downloads it (~66 s on this machine) into `~/.duckdb/extensions/v1.5.6/windows_amd64/`. Offline machines need it pre-installed, otherwise the adapter degrades (Intersects/Buffer/SpatialJoin disabled, advertised through `capabilities()`). Extension binaries from the Rust bundled build loaded fine (same platform tag).
4. DuckDB 1.5.6 reads GeoParquet geometry as a native `GEOMETRY('<projjson crs>')` column. Exporting that type through `query_arrow` in the Rust binding **fails** (`TransactionContext::ActiveTransaction called without active transaction`), so the adapter emits `ST_AsWKB(...)` and re-attaches GeoArrow extension metadata (`geoarrow.wkb` + CRS taken from the IR schema).
5. The DuckDB profiler's `total_bytes_read` **under-counts** (56 KB reported for a column scan of a 98 MB file) and `operator_rows_scanned` is not trustworthy (= rows × row groups). GeoFabric therefore derives rows scanned / row groups skipped from Parquet statistics analysis and reports DuckDB's bytes only as a relative signal. See the report for numbers.
6. Sandboxing works: after the extension is loaded the adapter sets `allowed_directories=[data roots]` and `enable_external_access=false`; an out-of-root read is rejected by DuckDB itself.

## Alternatives considered (not pursued)

- Dynamic/system DuckDB (`libduckdb` + `DUCKDB_LIB_DIR`): would cut build time and disk use but adds an install step and version-skew risk. Revisit if the 26 GB debug footprint stays a problem after the `debug = "line-tables-only"` change.
- Python DuckDB as engine: rejected by the project owner for the execution path.
