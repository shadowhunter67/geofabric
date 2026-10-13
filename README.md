# GeoFabric

> A spatial runtime for cloud-native geospatial computing: a logical Spatial IR, a planner, and replaceable execution engines.

Status: **core MVP, hardening round done** (see REPORT-GEOFABRIC-HARDENING.md) (vertical slice GeoParquet → Spatial IR → planner → DuckDB → Arrow → MapLibre). See [REPORT-GEOFABRIC-CORE-MVP.md](REPORT-GEOFABRIC-CORE-MVP.md) and [REPORT-GEOFABRIC-HARDENING.md](REPORT-GEOFABRIC-HARDENING.md) for what works, what does not, and the measurements. The real execution pipeline (and what is *not* claimed) is in [docs/pipeline.md](docs/pipeline.md).

Read first: [CONSTITUTION.md](CONSTITUTION.md) · [AGENTS.md](AGENTS.md) · [docs/vision.md](docs/vision.md) · [rfcs/](rfcs/) · [adr/](adr/)

## Layout

```
crates/geo-types            CRS, units, bbox, schema (no defaults: CRS/unit always explicit)
crates/geo-ir               logical Spatial IR, schema derivation, validation, JSON (versioned)
crates/geo-executor         ExecutionEngine contract + metrics types (RFC-002)
crates/geo-source-geoparquet  footer metadata, CRS, row-group stats, bbox pruning, Arrow reads
crates/geo-planner          rule-based planner v0 + decision log (RFC-003)
crates/geo-engine-duckdb    DuckDB adapter (IR -> SQL lives only here)
crates/geo-runtime          composition root: validate -> plan -> execute
crates/geo-gateway          HTTP API + static web UI (GeoJSON only at this boundary)
crates/geo-bench            benchmark runner
web/                        TypeScript + MapLibre UI (talks to the gateway only)
benchmarks/                 synthetic GeoParquet generator, results
tools/arch_check.py         architecture fitness check (dependency graph)
```

(Target tree in the brief uses `engines/`, `formats/`; the MVP keeps everything under `crates/` — see the report, "Architecture deviations".)

## Setup (Windows, one-time)

Prerequisites: Rust ≥ 1.99, Node ≥ 24, Python ≥ 3.12.

```
# synthetic data + Python tooling (venv is git-ignored)
python -m venv benchmarks/.venv
benchmarks/.venv/Scripts/python -m pip install pyarrow duckdb numpy
benchmarks/.venv/Scripts/python benchmarks/gen_synthetic.py --size 10mb  --out benchmarks/data
benchmarks/.venv/Scripts/python benchmarks/gen_synthetic.py --size 100mb --out benchmarks/data
# independent expected results (pyarrow + numpy only) for the benchmark correctness gate
benchmarks/.venv/Scripts/python benchmarks/oracle.py --size 10mb  --data benchmarks/data --out benchmarks/results
benchmarks/.venv/Scripts/python benchmarks/oracle.py --size 100mb --data benchmarks/data --out benchmarks/results

# web UI
cd web && npm install && npm run build && cd ..
```

First run of the DuckDB adapter downloads the `spatial` extension (needs network once, ~1 min).
The first DuckDB-bundled build takes 3–5 minutes and `target/` becomes large (ADR-006). Use `CARGO_BUILD_JOBS=3` to keep the machine responsive.

## Demo (exact steps)

```
cargo run -p geo-gateway -- --data benchmarks/data --port 8787     # serves API + web/dist
# open http://127.0.0.1:8787
```

1. The first source (`clustered_10mb_spatial`) is selected; its extent (orange) and row-group outlines (dashed) appear immediately.
2. Zoom the map into a cluster and press **Run** (Query tab = "Features in viewport").
3. **Execution Plan** tab: metadata read → spatial prune (row groups skipped, green = read, grey = pruned on the map) → planner decision log → generated DuckDB SQL → Arrow result → render, with metrics.
4. Click a feature: **Inspector** shows properties, source, CRS, approximate spherical area/perimeter and the lineage chain.
5. Compare `clustered_10mb_shuffled`: same answer, but nothing is pruned (0 / 26 row groups skipped).

UI development: `cargo run -p geo-gateway` + `cd web && npm run dev` (proxy to :8787).

## Checks

```
cargo test --workspace                 # needs benchmarks/data (tests skip with a message if absent)
python tools/arch_check.py             # dependency-boundary rules from the Constitution
cd web && npm run typecheck
cd web && npm run build && npx playwright test   # browser smoke (uses the installed Chrome, starts the gateway)
cargo build --release -p geo-bench
target/release/geo-bench.exe --data benchmarks/data --out benchmarks/results --sizes 10mb,100mb --reps 3 --tag run
```

## Trust model (v0)

The gateway binds to 127.0.0.1, has no authentication, and accepts **only logical IR or structured query templates — never SQL**. Source files must live under `--data`; remote URLs are refused; result rows, request size and concurrency are capped. Every query has an execution timeout (default 30 s, `--timeout-secs`) enforced *inside* DuckDB via `interrupt()`, and a client disconnect cancels the running query. Sources are authorized by a `SourceAccessGuard` before any file I/O; denials are opaque.

## Disk / editor tips

`target/` gets large (DuckDB bundled). Dev profile has no debug info (`debug = 0`). Optionally put it outside the workspace with `CARGO_TARGET_DIR=<dir outside the repo>` (not committed). `.vscode/settings.json` excludes `target/`, `node_modules/`, benchmark data from the watcher/search.
