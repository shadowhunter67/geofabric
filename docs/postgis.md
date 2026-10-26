# PostGIS in GeoFabric (optional engine)

Design and decisions: `adr/ADR-013-postgis-engine.md`. Architecture: `docs/architecture/engine-postgis.md`. Status: **implemented and verified against a live PostgreSQL 16.9 / PostGIS 3.6.2** (2026-10-21; evidence: `REPORT-GEOFABRIC-POSTGIS-LIVE-VERIFICATION.md`). No cost model yet: the planner never picks it on predicted cost (R5d).

## What it is

An additional *execution engine* for plans whose data is resident in a PostgreSQL/PostGIS database. GeoFabric works exactly as before without it (no configuration = no engine, no probe, no error). Normal users (Explorer mode) never see it; Developer mode shows `PostGIS: not configured` or its version/health, and lists it as a planner candidate with its reason when it is configured.

## Versions (pinned, never "latest")

| component | version | source |
|---|---|---|
| PostgreSQL | 16.9 (minimum accepted by the probe: 14) | EnterpriseDB portable zip (`postgresql-16.9-1-windows-x64-binaries.zip`) |
| PostGIS | 3.6.2 (minimum: 3.3) | osgeo.org bundle `postgis-bundle-pg16-3.6.2x64.zip`, vendor md5 verified; SHA-256 pinned in `tools/postgis.lock.json` |

Integrity wording (exact): PostgreSQL = *trusted on first acquisition over HTTPS; locally pinned SHA-256*. PostGIS = vendor checksum (md5) verified, plus the same locally pinned SHA-256. Redirects are refused; sizes are pinned.

Native alternatives (an existing PostgreSQL ≥ 14 with PostGIS ≥ 3.3, Docker, Podman) work the same way: only the environment variables below matter. Docker/Podman/WSL were not present on the development machine.

## Setup (portable, no admin rights, no service)

```
python tools/postgis_portable.py setup     # download + verify (md5 / recorded sha256), extract, initdb (UTF8, C locale), roles, extension
python tools/postgis_portable.py start     # 127.0.0.1:5433
python scripts/postgis/load_data.py --parquet data/local/overture_places_hcm.parquet --table places --crs OGC:CRS84 \
       --logical-uri <the uri plans use>   # table + GiST index + ANALYZE + parity check + registers the source
python tools/postgis_portable.py stop      # stops only the server this script started (PID from postmaster.pid)
```
Roles: `gf_admin` (owner; setup/import) and `gf_reader` (SELECT only; what the gateway uses). The runtime directory is `%LOCALAPPDATA%\geofabric-runtime\postgres` (override: `GEOFABRIC_RUNTIME_DIR`): it must be an ASCII path outside the repo, because `initdb` fails on a path with Vietnamese diacritics. Local development passwords are generated into `credentials.json` there (never in the repo); nothing secret is committed. The port is bound to 127.0.0.1 and must stay there (the browser never talks to PostgreSQL).

## Configuration

| variable | meaning |
|---|---|
| `GEOFABRIC_POSTGIS_ENABLED` | `1`/`true` to enable; anything else = disabled |
| `GEOFABRIC_POSTGIS_URL` | `postgres://gf_reader:<password>@127.0.0.1:5433/geofabric` (never printed) |
| `GEOFABRIC_POSTGIS_SOURCES` | path of the sources catalog written by `load_data.py` (`data/manifests/postgis-sources.json`) |
| `GEOFABRIC_POSTGIS_POOL` | pool size, 1–32 (default 4) |

The driver is a cargo feature: `cargo build --release -p geo-gateway --features postgis-live`. Without it a configured engine exists but reports "no PostgreSQL driver in this build" (visible in Developer mode, never silent). No TLS in the driver: local development only.

## Capabilities

Source, Filter, BboxFilter, Intersects, Transform, SpatialSemiJoin, Aggregate, Project, Limit, Sort. Not supported (the planner excludes the engine before execution): Buffer, SpatialJoin, render sampling. Transactions exist in PostgreSQL but are not exposed (read-only adapter).

## Semantics that must not drift (see ADR-013)

OGC:CRS84 → SRID 4326, coordinates (lon, lat); CRS84 and EPSG:4326 are different CRSs semantically; metric distances = `ST_Transform` to UTM + planar `ST_DWithin` inside `EXISTS` (each left row once); never `geography`; `NULLS LAST`, `COLLATE "C"`, `ST_AsBinary(.., 'NDR')`.

## Tests

* Without a database (part of `cargo test --workspace`, 34 tests): `crates/geo-engine-postgis/{src,tests}` (config redaction, catalog, CRS mapping, probe evaluation, SQLSTATE mapping, lowering goldens + PostgreSQL-grammar syntax check of every statement, injection, refusals, capability matrix, engine behaviour with a mock connector) and `crates/geo-runtime/tests/postgis.rs` (planner + runtime + health with the real DuckDB engine and a mock PostGIS).
* With a database: `GEOFABRIC_POSTGIS_URL=… [GEOFABRIC_POSTGIS_RO_URL=…] cargo test -p geo-engine-postgis --features live --test live` (8 tests: probe, bound/hostile parameters, failure taxonomy, read-only account, server-side timeout, server-side cancellation, bounded pool). Cross-engine: `cargo test -p geo-runtime --no-default-features --features postgis-live --test postgis_live` (17 plans; `GF_XE_*` variables name the data; `GF_BENCH=1` runs the benchmark). Without the URL these tests print SKIP and pass: a skip is not evidence, so report live and offline results separately.
* Cross-engine correctness (needs the data loaded): run the same plan through `POST /api/query` with `"engine":"duckdb"` and `"engine":"postgis"` and compare `result_digest`; Demo B must give the same 27,624 ids as `python benchmarks/oracle_near.py`. Geometry comparison must use topological/normalized equality where byte equality is not stable.

## What is verified, what is not

Verified without a database: lowering shape and syntax, parameterization, refusals, authorization order, error mapping, health blame, planner exclusion, R5d, graceful absence, arch rules. Verified live (2026-10-21): see ADR-013 "Live verification". **Not done**: a measured PostGIS cost model (R5d stays), TLS, a second machine, a dataset that exists only in the database.

## Licences

Data imported from Overture (ODbL) keeps its attribution obligations in the copy (`docs/DATA-LICENSES.md`).

## DuckDB profiler (opt-in)

The DuckDB engine's JSON profiler is **off by default** and only on when `GEOFABRIC_DUCKDB_PROFILE` is exactly `1` (the benchmark tool sets it; it runs one query at a time). With it on, several engines in one process failed sporadically in CI ("Error code 1"); with it off, `peak_memory_bytes` is `Unknown` with an explanatory note. The contract is unit-tested (`profiling_contract` in `geo-engine-duckdb`).

## Cost model, risk policy and when PostGIS is chosen automatically (ADR-015)

`PostGisEngine::cost_estimate` classifies every plan from its plan-time features and prices it with an interpretable per-class formula (`crates/geo-engine-postgis/src/cost_model_v1.json`: coefficients with their provenance). The planner chooses PostGIS **on predicted cost only in a validated class with an available index**; elsewhere policy R5d keeps the measured engine, and an explicit `"engine":"postgis"` still selects it.

| Workload class | PostGIS model | Automatic choice |
|---|---|---|
| selective indexed window (`indexed_bbox`) | validated | yes |
| intersects / projected column (`indexed_spatial`) | limited (passed one independent set, failed another) | no |
| aggregates up to 300,000 matching rows (`aggregate`) | validated | yes |
| admin-style intersects semi join (`semijoin_intersects`) | validated | yes |
| broad scans (> 300,000 rows read) | limited (passed one independent set, failed another) | no |
| metric semi join, <= 5,000 probes | limited | no |
| metric semi join, > 5,000 probes | limited | no; refused for automatic choice above 50,000 probes (hard risk policy) |

The index state comes from the catalog written by `load_data.py` (`spatially_indexed`, `metric_indexes`); `unknown` is never treated as available, and an unindexed source is never `validated`. Developer mode shows, per candidate, the predicted time, class, validation level, risk, index state and the largest terms.

Scope of the evidence: one machine, loopback PostgreSQL 16.9 + PostGIS 3.6.2, 3.85 M buildings. It does not cover a remote database, another host or unknown index state.

### DuckDB profiler (opt-in)

The DuckDB engine's JSON profiler is **off by default** and on only when `GEOFABRIC_DUCKDB_PROFILE` is exactly `1` (the benchmark tool sets it; it runs one query at a time). With it on, several engines in one process failed sporadically in CI ("Error code 1"); with it off, `peak_memory_bytes` is `Unknown` with an explanatory note. The contract is unit-tested (`profiling_contract` in `geo-engine-duckdb`).

### Re-running the evidence

```
python benchmarks/make_postgis_cost_cases.py --split calibration|heldout|confirm --out benchmarks/results/postgis-cost/cases-<split>.json
GF_COST_CASES=... GF_COST_OUT=benchmarks/results/postgis-cost/raw/<split>.jsonl \
  cargo test --release -p geo-runtime --no-default-features --features postgis-live --test postgis_cost -- --nocapture     # opt-in, live, resumable
python benchmarks/postgis_cost_fit.py fit --cal <raw sets> ...      # coefficients (calibration data only)
python benchmarks/postgis_cost_eval.py decide|report ...             # gates in benchmarks/postgis_cost_gates.json
```
`GF_COST_PLAN_ONLY=1` re-plans stored cases with the current models without executing anything. Raw traces (EXPLAIN text, every run) stay out of Git (`benchmarks/results/postgis-cost/raw/`); the compact per-case files and the evaluations are committed.

**Update 2026-10-26 (model `postgis-cost-v2`, ADR-015 addendum).** The table above is the current state: after a connector latency bug (+~15 ms on every PostGIS query) was fixed and everything re-measured, three classes are validated on two independent sets (`indexed_bbox`, `aggregate`, `semijoin_intersects`); the rest stay on policy R5d. Indexed queries now run in 1-3 ms. The planner adds a *scale warning* (not a refusal) for metric semi-joins above 250,000 probes.

## Metric semi-join safety (2026-10-26; ADR-015 addendum 2)

* **Features.** For a metric semi join the planner estimates, before anything runs and from the spatial histogram + column statistics: expected probes, the right side after its attribute filter (`right_rows_filtered`), the right-side density per km2, candidates per probe (`density x pi x d^2`), **candidate pairs** (`probes x candidates per probe`) and the window overlap ratio. A value the planner cannot estimate is `null` and the models answer `unknown` for it. With histograms the left-side estimate has P90 error x1.5 (x5.3 without) and the right side x4.5 (x13); the Developer panel prints the features and names the unknown ones.
* **Classes.** `semijoin_metric_small` (<= 5,000 probes), `semijoin_metric_medium` (<= 50,000 probes and < 1e7 candidate pairs), `semijoin_metric_large` (everything else, including unknown pairs). Thresholds were set from the discovery data before the calibration and confirmation sets were run.
* **DuckDB timeout risk.** All 10 observed DuckDB timeouts (60 s) have 7.8e7 or more candidate pairs and a 500-1000 m radius. `duckdb-timeout-risk-v1` is a log-linear Tobit model for the regime >= 1e7 pairs; plans at P >= 0.10 are `elevated`, at P >= 0.20 `timeout_risk`. PostGIS never timed out in ~700 metric runs (slowest 42 s): its risk model (`postgis-timeout-risk-v1`) is the same kind, and stays `low` inside the measured envelope.
* **Policy.** R5e (see RFC-003): a DuckDB plan at timeout risk is not kept just because PostGIS is unvalidated for its class; PostGIS' 50k-probe block is lifted in exactly that situation; if both are at risk the planner says it is uncertain. Explicit engine requests are unaffected.
* **Operations.** `Runtime::ensure_histogram` builds/loads the histogram sidecars (the gateway does it in the background; the benchmark harness does it up front, so the benchmark measures the planner that ships). `benchmarks/run_cost_set.py` records the host's RAM-watchdog state and classifies an abnormal exit as `HOST_RESOURCE_KILL` (the watchdog's log names the benchmark process) or `ENGINE_CRASH`. The full-stack suite refuses a stale gateway binary (`/api/health` reports the build identity; `web/e2e/global-setup.ts`).
