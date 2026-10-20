# PostGIS in GeoFabric (optional engine)

Design and decisions: `adr/ADR-013-postgis-engine.md`. Architecture: `docs/architecture/engine-postgis.md`. Status: **implemented and tested without a database; never run against a live server** (see "What is verified" below).

## What it is

An additional *execution engine* for plans whose data is resident in a PostgreSQL/PostGIS database. GeoFabric works exactly as before without it (no configuration = no engine, no probe, no error). Normal users (Explorer mode) never see it; Developer mode shows `PostGIS: not configured` or its version/health, and lists it as a planner candidate with its reason when it is configured.

## Versions (pinned, never "latest")

| component | version | source |
|---|---|---|
| PostgreSQL | 16.9 (minimum accepted by the probe: 14) | EnterpriseDB portable zip (`postgresql-16.9-1-windows-x64-binaries.zip`) |
| PostGIS | 3.6.2 (minimum: 3.3) | osgeo.org bundle `postgis-bundle-pg16-3.6.2x64.zip`, md5 `6730107131c29b7ff9a36702f736007b` |

Native alternatives (an existing PostgreSQL ≥ 14 with PostGIS ≥ 3.3, Docker, Podman) work the same way: only the environment variables below matter. Docker/Podman/WSL were not present on the development machine.

## Setup (portable, no admin rights, no service) — NOT RUN YET

```
python tools/postgis_portable.py setup     # download + verify (md5 / recorded sha256), extract, initdb (UTF8, C locale), roles, extension
python tools/postgis_portable.py start     # 127.0.0.1:5433
python scripts/postgis/load_data.py --parquet data/local/overture_places_hcm.parquet --table places --crs OGC:CRS84 \
       --logical-uri <the uri plans use>   # table + GiST index + ANALYZE + parity check + registers the source
python tools/postgis_portable.py stop      # stops only the server this script started (PID from postmaster.pid)
```
Roles: `gf_admin` (owner; setup/import) and `gf_reader` (SELECT only; what the gateway uses). Local development passwords are generated into `tools/.cache/postgres/credentials.json` (gitignored); nothing secret is committed. The port is bound to 127.0.0.1 and must stay there (the browser never talks to PostgreSQL).

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
* With a database: `GEOFABRIC_POSTGIS_URL=… cargo test -p geo-engine-postgis --features live --test live` (probe, bound parameters, server-side timeout, server-side cancellation, bounded pool). Without the URL these tests print SKIP and pass — a skip is not evidence.
* Cross-engine correctness (needs the data loaded): run the same plan through `POST /api/query` with `"engine":"duckdb"` and `"engine":"postgis"` and compare `result_digest`; Demo B must give the same 27,624 ids as `python benchmarks/oracle_near.py`. Geometry comparison must use topological/normalized equality where byte equality is not stable.

## What is verified, what is not

Verified (no database): lowering shape and syntax, parameterization, refusals, authorization order, cancellation before the call, row cap, error mapping, health blame, planner exclusion by residency/capability, R5d policy, graceful absence, arch rules. **Not verified**: that a real PostgreSQL/PostGIS accepts and answers the SQL; row/geometry parity with DuckDB; Demo B on PostGIS; index use; performance; the live driver (pool, statement timeout, cancel); the setup and import scripts. Do not treat any of these as done.

## Licences

Data imported from Overture (ODbL) keeps its attribution obligations in the copy (`docs/DATA-LICENSES.md`).
