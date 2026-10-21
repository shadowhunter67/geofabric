# GeoFabric — PostGIS live verification (2026-10-21)

Base commit `127d0e5`, branch `main`. Machine: one Windows 11 box (32 GB). Everything below was run against a real PostgreSQL/PostGIS on `127.0.0.1:5433`. Evidence files are in `benchmarks/results/postgis/`.

## Executive Summary

**Recommendation: `POSTGIS ENGINE: APPROVED` (correctness and contract), with one standing restriction: the engine stays `Unmeasured` and R5d stays in force.** Every correctness gate passed on live data. The live run found five real defects that the offline suite could not (BboxFilter meaning, per-row semi-join probe, URI normalization, async-runtime panic of the sync driver, partial-copy coverage); all are fixed and each has a test. Performance is workload dependent in both directions: PostGIS is 5–30x faster on selective indexed windows, DuckDB is ~10x faster on large metric semi-joins (Demo B on a 535 k-building window: 1.0–1.9 s vs 12–17 s). A single cost factor would be wrong, so the planner must not choose PostGIS on predicted cost yet.

## Portable Runtime

`tools/postgis_portable.py` (download / setup / start / stop / status). Runtime dir `%LOCALAPPDATA%\geofabric-runtime\postgres` (ASCII, outside the repo: `initdb` fails on a path with Vietnamese diacritics — reproduced). No Docker/WSL/service/PATH change/admin. `stop` goes through `pg_ctl` using the PID in `postmaster.pid`; the PID was logged (25476, 27448, …); no process was killed by image name. Listens on `127.0.0.1` only (`SHOW listen_addresses`); no firewall change.

## Download Integrity

| artifact | size | check |
|---|---|---|
| `postgresql-16.9-1-windows-x64-binaries.zip` (EnterpriseDB) | 314,500,604 | **trusted on first acquisition over HTTPS; locally pinned SHA-256** `304eb7da…d88db88` |
| `postgis-bundle-pg16-3.6.2x64.zip` (osgeo.org) | 123,957,610 | vendor md5 verified; locally pinned SHA-256 `9f4e8a30…4ff2a6` |

Redirects are refused, sizes pinned, lock in `tools/postgis.lock.json` (committed; binaries are not).

## Observed Versions

PostgreSQL 16.9 (Visual C++ 1943, 64-bit); PostGIS 3.6.2 (GEOS 3.14.1dev, PROJ 8.2.1, `NETWORK_ENABLED=OFF`). Both equal the pinned versions.

## PostgreSQL Configuration

UTF8, C collation; `port=5433`, `listen_addresses=127.0.0.1`, `max_connections=40`, `shared_buffers=512 MB (65536×8k)`, `work_mem=64 MB`, `effective_cache_size=4 GB`, `max_parallel_workers_per_gather=2`, `jit=on`, `random_page_cost=4`. These are modest defaults chosen once; **not tuned per benchmark** (no gaming). `jit=on` was left at the default and not evaluated.

## Accounts / Permissions

`gf_admin` (superuser of this private instance; setup/import only) and `gf_reader` (not superuser, `SELECT` only; the runtime account). Live test `the_runtime_account_cannot_write`: `CREATE TABLE`, `DELETE`, `UPDATE`, `DROP TABLE` as `gf_reader` are all refused with SQLSTATE 42501. Passwords live in the runtime dir; none is committed or printed; a refused connection's error does not contain the password (test).

## Runtime Probe

Real probe returns reachable, PostgreSQL 16.9, PostGIS 3.6.2, required functions present (`ST_Transform`, `ST_DWithin`, `ST_AsBinary`, GiST) → `PostGisProbe.ok`. With the server stopped it reports `not reachable: error connecting to server` (no credential).

## Live Fixture

Deterministic tiny fixture (`scripts/postgis/make_fixture.py` → `fx_buildings`, `fx_roads`, `fx_admin`), loaded with parity checks. All 8 live tests pass (`cargo test -p geo-engine-postgis --features live --test live`).

## Parameterization

Live: `x'; DROP TABLE geofabric.fx_buildings; --`, `O'Brien`, `"quoted"`, a backslash, `Đường Nguyễn Huệ — Quận 1 ✓`, `', (SELECT 1)) --`, `%_like`, empty string — each round-trips byte for byte as a bound `$n::text`, and the fixture table is intact afterwards. Offline: every generated statement parses with a PostgreSQL grammar and contains no literal values.

## Cancellation

`cancelling_the_context_cancels_the_statement_on_the_server`: a `pg_sleep` statement disappears from `pg_stat_activity` after the context is cancelled (server-side proof, observed through a second connection), not merely "the call returned".

## Timeout

`the_server_enforces_the_statement_timeout`: `SET statement_timeout` is applied; the statement is stopped by the server; the error is classified `Timeout` (not `Cancelled`).

## Connection Pool

`the_pool_is_bounded_and_connections_are_reused`: pool size 2; six concurrent 0.3 s statements all succeed (the extra ones wait for a free connection) while a second connection observing `pg_stat_activity` never sees more than 2 of them active at once (and sees at least 1, so the check is not vacuous). The pool-exhausted `Timeout` path (`no database connection became free before the timeout`) is covered offline only, not live. Failure taxonomy live: missing relation → `Relation/42P01`; missing column → `Relation/42703`; missing function → `Capability`; `1/0` → `Other`; refused connection → `Connection` (password absent from the message).

## Data Import

`scripts/postgis/load_data.py`: geometry loaded as SRID 4326 (lon, lat), covering columns stored as float8, GiST on geometry, GiST on the covering envelope (functional), GiST on the UTM-48N transform (`*_utm32648_gix`) for metric predicates, `ANALYZE`. Loaded: buildings 3,845,734 (clipped to `[106.35,10.35,107.05,11.25]`, recorded as `coverage` so it cannot answer for the whole dataset), places 449,849, transport 453,550, landuse 13,277, admin 325. Database 1,786 MB. The transport table also carries an experimental generated metric column used while measuring; it is not referenced by the lowering.

## Data Parity

Strict, per table (`parity-hcmc-*.json`): row count equal, distinct id sets equal, null geometries equal (0), geometry types equal, SRID 4326, order-independent geometry digest equal, **bit-exact covering digest equal** — for all five datasets. The fixture and the 4 slice tables passed the same checks.

## Spatial Indexes

Per table: geometry GiST, covering-envelope GiST, UTM GiST (buildings, transport). Names in `pg_indexes` (`buildings_geometry_gix`, `buildings_covering_gix`, `buildings_utm32648_gix`, `admin_geometry_gix`, `admin_covering_gix`, …).

## ANALYZE

Run after each load (statistics present; row estimates feed the catalog `row_estimate` and the planner's `SourceEstimated` metric). EXPLAIN row estimates were in the right order of magnitude for the small windows; the metric semi-join estimate on the wide window was off by a large factor (see EXPLAIN Evidence).

## Cross-Engine Correctness

The same 17 plans (limit, sort with NULLS/ties, text sort, filters incl. IN/IS NULL, numeric+NOT, aggregates, count_by with null group, Intersects polygon, Transform column, two metric semi-joins, Demo B count/ids in the gateway template shape, Intersects semi-join with admin) through `Runtime::dispatch` on DuckDB and on PostGIS. Comparison: row count, ordered rows, canonical SHA-256 digest of the result, coordinate-wise geometry comparison with tolerance `1e-9` where geometry is returned. Result: **all identical** in `crosscheck-slice.json` (17/17), `crosscheck-full-demob-window.json` (17/17, full 3.85 M buildings), `crosscheck-full-wide.json` (16/16, 535 k buildings in the window; the admin semi-join is skipped there because DuckDB takes minutes).

## Metric Semantics

Metric = `ST_Transform` to UTM 48N + planar `ST_DWithin` inside `EXISTS`; no `geography`, no degree approximation. 200 m and 50 m semi-joins agree row for row with DuckDB (41,150 / 40,997 in the Demo B window; 534,511 / 531,759 in the wide window).

## SpatialSemiJoin

Each left row appears once (EXISTS). The live run showed that the right input is probed per left row: with the envelope index assist on the inner side the planner repeated the covering probe 41,150 times (10.4 s); omitting only that assist (the exact comparisons still decide) gives the same rows in 0.9 s. Intersects semi-join with admin: 41,150 rows identical; PostGIS 0.17–0.25 s vs DuckDB 50–57 s (full file scan, no spatial index).

## Demo B

Template shape exactly as the gateway builds it (window, metric copy, expanded right window, class filter `motorway,trunk,primary`, Project, Transform replacing the geometry, 200 m). Demo B window: **27,624 ids**, equal on both engines. Wide window: 189,772 ids, equal.

## Independent Oracle

`benchmarks/oracle_near.py` (shapely STRtree + pyproj, no DuckDB, no PostGIS). Demo B window: 27,624 ids; wide window: buildings 534,572, roads 5,825, near count 189,772, ids SHA-256 `f7828710…ef687`. The PostGIS id set, hashed in the test (sorted ids, SHA-256), equals the oracle's in both windows (`oracle_ids_sha_match: true`). Note: an earlier 27,634 reading came from a stale release gateway, not from either engine — fixed by rebuilding.

## Full HCMC Data

Buildings 3,845,734 / places 449,849 / transport 453,550 / landuse 13,277 / admin 325 loaded with the strict parity above. The wide-window cross-check is the largest equal-result comparison (535 k-building scan on both engines).

## EXPLAIN Evidence

Dev/benchmark only (`benchmarks/postgis_explain.py`, `EXPLAIN (ANALYZE, BUFFERS)` of PREPAREd statements; never in the product path). Small window: 1.1–1.8 ms execution, `Bitmap Index Scan` on the covering GiST for bbox/filter/aggregate/Intersects; Demo B ids 94 ms (`Index Scan on transport_utm32648_gix`, nested loop); admin semi-join 26 ms (`admin_geometry_gix`). Wide window: bbox+sort+limit 149 ms, filter 72 ms, aggregate 83 ms, Intersects 187 ms, **Demo B ids 15.4 s** (nested loop over 190 k probes, index scan per probe), admin semi-join 1.4 s.

## Index Behaviour

Selective windows use the indexes as intended. Large windows lose: the planner correctly estimates that 535 k buildings are not selective, but the metric semi-join remains a nested loop of per-row index probes, which is where DuckDB's hash/partitioned spatial join is far ahead. Not "fixed" by tuning in this phase (no evidence-free optimization).

## Benchmark Method

Wall clock around `Runtime::dispatch` (plan + authorization + execution + Arrow result), identical for both engines, same plans, same machine, results asserted equal on every run. **first** = first execution of the case in the process; PostgreSQL is restarted before each window, so its shared buffers are cold but **the OS file cache is warm** (it cannot be dropped without admin) — "cold database cache, warm OS cache"; DuckDB's first run includes per-process state. **warm** = median of 5 repetitions (3 when first <10 s, 1 when slower). Three windows: small (≈1 km²), medium, wide (535 k buildings). Not a load test; single client.

## Cold / Warm Results

Warm medians, ms (DuckDB / PostGIS), `bench-*.json`, `bench.log`:

| case | small | medium | wide |
|---|---|---|---|
| filter_eq_text | 228 / 33 | 225 / 49 | 490 / 117 |
| aggregate_count_star | 190 / 32 | 405 / 82 | 205 / 79 |
| intersects_polygon | 210 / 33 | 393 / 70 | 335 / 378 |
| filter_in_or_isnull | 273 / 36 | 226 / 115 | 790 / 1836 |
| transform_to_utm48_column | 213 / 33 | 477 / 351 | 673 / 1331 |
| semi_join_metric_200m | 280 / 110 | 3529 / 1710 | 27000 / 25769 |
| semi_join_metric_50m | 269 / 188 | 2588 / 2554 | 12181 / 43486 |
| demo_b_ids_template_shape | 232 / 96 | 338 / 1001 | 1030 / 14217 |
| semi_join_intersects_admin | 1915 / 50 | (skipped) | (skipped) |

First-touch numbers are within noise of warm for DuckDB; for PostGIS they differ by at most one cache fill (`limit` 34 vs 33, `bbox_sort_limit` 67 vs 50 in medium). DuckDB shows a ~190–250 ms per-query floor (process/extension/metadata overhead in this embedding) that dominates small windows; this is a property of how it is run here, reported as measured, not a claim about DuckDB in general.

## Planner / R5d

Unchanged: PostGIS reports `Unmeasured`; R5d keeps measured engines first; only an explicit `preferred_engine` or being the only feasible engine selects it (offline-verified). R5b/R5c untouched. No planner code changed this phase.

## PostGIS Calibration

**Not done, deliberately.** The data show the cost ratio is not a constant: 0.1–0.2x on selective windows, 0.5–1x on medium, 3–14x on large metric semi-joins. A one-factor calibration would hide that. The next phase should give the PostGIS estimate selectivity and semi-join shape as inputs, fit on a calibration set, and validate on a held-out set before R5d is relaxed.

## UI / Health

`/api/state.postgis` reports `configured` + probe (version or the failed check), never the URL. Checked live through the API only: with the server down the probe says unreachable (observed). The Developer-view candidate row and the R5d reason with the server up were verified offline (mock) and were not re-inspected in a browser against the live server in this phase. Runtime health blames the engine that failed (offline test with mock: three connection failures → Unhealthy and the planner routes around it).

## Optional-Unavailable Behaviour

PostgreSQL stopped (`postgis_portable.py stop` → `not running`), release gateway built with `postgis-live`, PostGIS enabled but unreachable: the gateway starts, logs `PostGIS engine registered (probe: … not reachable …)` (no credentials), `/api/health` ok, `/api/sources` lists 4 sources, `/api/query` (explicit `duckdb` and automatic) returns counts, the UI serves 200. No PostGIS configuration at all = no engine, no probe, no error (offline-verified).

## Tests

Live (separate from offline; each needs the portable server):
* `geo-engine-postgis --features live --test live`: **8/8 pass**, including the read-only-account test run with `GEOFABRIC_POSTGIS_RO_URL`.
* `geo-runtime --features postgis-live --test postgis_live`: cross-engine (17 plans) and benchmark — pass (see Cross-Engine Correctness).
Offline (no database, mock connector; skips are not counted): `cargo test --workspace` **228 passed, 0 failed**; web unit tests `npm test` **76 passed**; `tsc --noEmit` clean; `tools/arch_check.py` OK (8 crates, no boundary violations). Playwright (release gateway built with `--features postgis-live`, PostGIS not configured): Edge full suite **61 passed, 0 failed, 4 skipped** on a fresh gateway; Chrome **55 passed + the 7 browser-engine specs re-run to green = 62, 3 skipped** (the first Chrome pass failed those 7 because I had started the gateway with `--no-browser`; the first Edge pass failed 5 because the long-lived gateway had learned an "unhealthy browser" from an earlier crash-report test — both were harness mistakes, not product defects, and both disappeared with a correctly started fresh gateway). The skipped specs are the opt-in screenshot/perf ones (`UI_SHOTS`, pack-perf) and are not counted as evidence.

## ADR Status

ADR-013 → **Accepted** (2026-10-21), with the live-verification section and the corrected BboxFilter/semi-join contract.

## Architecture Risks

1. Three independent lowering compilers (Rust/DuckDB, TypeScript, Rust/PostGIS); the live run proved they can drift (BboxFilter). The covering-box fix is covered by a test, but the structural fix is RFC-004's shared SQL intent.
2. Cost model absent while the ranking flips by workload: a mis-set model could route large metric joins to the slow engine.
3. Large metric semi-joins on PostGIS are a nested loop of probes; there is no guard that refuses or warns for such plans on PostGIS.
4. Single machine, single client, OS cache warm: not a concurrency or multi-user result.
5. Catalog residency is a local JSON file; a dataset that exists only in PostGIS is not plannable yet (footer read).
6. No TLS in the driver (localhost only).

## Remaining P0

None for correctness. Gate for *using* PostGIS automatically: a measured, held-out-validated cost model (until then R5d).

## Recommended Next Phase

(1) PostGIS cost model with selectivity + semi-join-shape inputs, calibration set and held-out validation, then re-evaluate R5d; (2) a plan-shape guard (or cost term) for per-row-probe semi-joins on PostGIS; (3) only then the shared-lowering (RFC-004) refactor, now with a live PostGIS as the regression oracle. Do not start either before Principal Architect review.
