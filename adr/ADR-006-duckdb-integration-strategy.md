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
6. Sandboxing works: after the extension is loaded the adapter sets `allowed_paths=[the authorized files]` (+ a private temp dir for profiler output) and `enable_external_access=false`; an out-of-root read is rejected by DuckDB itself.

## Alternatives considered (not pursued)

- Dynamic/system DuckDB (`libduckdb` + `DUCKDB_LIB_DIR`): would cut build time and disk use but adds an install step and version-skew risk. Revisit if the 26 GB debug footprint stays a problem after the `debug = "line-tables-only"` change.
- Python DuckDB as engine: rejected by the project owner for the execution path.

## Update 2026-10-13 (hardening): two-mode strategy and evaluation

| topic | bundled (default) | dynamic / prebuilt (`DUCKDB_DOWNLOAD_LIB=1`, no default features) |
|---|---|---|
| clean build (this machine, jobs=3) | ≈10 min workspace tests, 7.6 min release bench | **54 s** for the adapter crate |
| `target/` size | tens of GB (59.6 GB filled the disk; now `debug = 0`) | **747 MB** |
| runtime dependency | none (static) | `duckdb.dll` (36.7 MB) beside the exe / on PATH |
| version pinning | exact: crate `libduckdb-sys 1.10506` = DuckDB 1.5.6, built from source | exact: build script downloads the matching official release for 1.5.6 |
| reproducibility | high (source build, offline after `cargo fetch`) | depends on the release asset being available and unchanged; needs network at build time |
| supply chain | compiles vendored source | downloads a binary at build time (verify checksum / mirror for release) |
| spatial extension | runtime `INSTALL`/`LOAD`, version must equal the library (`v1.5.6`) — same in both modes | same |
| developer setup | slow first build, big disk | fast; one extra DLL |
| status | used for **all** tests and benchmarks so far | works for the adapter + probe (loads, spatial=true); **not run against the full suite**; features are not yet forwarded through runtime/gateway/bench |

**Strategy (Proposed):** development → prebuilt/dynamic once feature forwarding is added and the suite passes with it; CI and release → bundled with pinned versions (reproducible, no download at build time). Python DuckDB stays tooling-only (dataset generation, never on the execution path; the benchmark oracle does not use it at all). Other 2026-10-13 findings: with debug info, test binaries linking bundled DuckDB exceed the MSVC PDB limit (LNK1140) → `[profile.dev] debug = 0`; a debug+release `target/` reached 59.6 GB and filled the disk.

## Update 2026-10-13 (multi-engine phase): prebuilt validated for development

**Decision (still Proposed for CI/release):** development default = the verified prebuilt `libduckdb` linked dynamically; CI/release = bundled with pinned versions.

How it works: `python tools/duckdb_prebuilt.py` derives the DuckDB version from `Cargo.lock` (`libduckdb-sys 1.10506.0` -> v1.5.6), downloads `libduckdb-windows-amd64.zip`, verifies its **sha256 against the digest GitHub publishes for the asset *and* against `tools/duckdb-prebuilt.lock.json`** (`44cf5958…fe96a`), refuses path-traversing archive entries, and extracts to `tools/.cache/` (git-ignored). Then `export DUCKDB_LIB_DIR=…; export PATH="$DUCKDB_LIB_DIR:$PATH"; cargo test --workspace --no-default-features` (features are forwarded through runtime/gateway/bench; `bundled` stays the default). The spatial extension is still `INSTALL`/`LOAD`ed at runtime and must be v1.5.6 as well.

Evidence on the prebuilt library: full Rust suite passes (clean build + all tests ≈ 61 s versus ≈ 10 min bundled); gateway tests incl. timeout/cancel pass; benchmark validation against the independent oracles passes for 10 MB, 100 MB, **1 GB and the real Overture subset** (all 91 runs `pass`); a release `geo-bench`+`geo-gateway` build takes ≈ 2 min; `target/` stays small (hundreds of MB instead of tens of GB). The prebuilt library is an optimized official build, which also makes debug-profile timings meaningful.

Not verified this round: the **bundled** path (CI/release) was not rebuilt or re-run, so "CI/release = bundled" is still the intended strategy, not an exercised one. Version pinning is by `Cargo.lock` + the lock file; the DLL must be on `PATH` (or next to the executable) when running.
