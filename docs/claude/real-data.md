# Vietnam demo data pack (2026-10-16) - working notes

Report: `REPORT-GEOFABRIC-REAL-DATA-INTEGRATION.md`. Licences: `docs/DATA-LICENSES.md`.

## Commands (repo root; Python = `benchmarks/.venv/Scripts/python.exe`: duckdb + pyarrow)
- `python data/scripts/geo_data.py list | validate | verify [id] | report [id] | seal <id> <paths>`
- `python data/scripts/geo_data.py bootstrap "Ho Chi Minh City" --run [--only id ...] [--dry-run]` -> roads, places, boundaries, land use, DEM (~0.2 GB). Skips what already verifies.
- `... bootstrap "Southern Vietnam" --run` -> Overture buildings (1.45 GB, ~15 min, S3 anonymous). Region names are the manifests' `coverage.name`.
- Gateway: `--data data/local --data benchmarks/data --manifests data/manifests --repo-root .` (several `--data`, first wins). `start-ui.bat` already passes them.
- Tests: `python -m unittest discover -s data/scripts/tests` (offline), `cargo test -p geo-gateway --no-default-features`, `npx playwright test data-pack map` (needs the pack, skips otherwise). Perf evidence: `PACK_PERF=1 npx playwright test pack-perf`.

## Facts that cost time
- Overture admin units here: province (region) + ward/commune (locality) + microhood/neighborhood; NO district tier (Vietnam's 2025 reform). Land and territorial-water rows are separate (`is_land`); the layer excludes `country` and water rows.
- Overture buildings: height, num_floors and every roof/facade field are 100 % null in this subset (see `data/quality/`). Nothing is invented; the Explorer inspector omits null rows.
- Overture land use had one 347 x 26 degree "marine national monument" polygon: excluded by an extent guard in the preset (documented in the manifest notes).
- CARTO free raster tiles now return an "API KEY REQUIRED" watermark; basemap = OpenFreeMap vector style (no key).
- OpenFreeMap's style puts a water label BEFORE roads and buildings: insert our layers before the first symbol layer AFTER the last geometry layer, not before the first symbol layer.
- IR: `Aggregate` drops geometry and cannot group by it, so "buildings near roads" = `near` (pairs) + `near_count` (distinct ids, two-level aggregate). Buffer on lon/lat uses an explicit ANGULAR distance (metres / 111320); error E-W about 1.9 % at 10.8 N. The gateway reports it in `approximation`. Proper metre buffers need a projected-CRS/Transform design (open RFC question).
- Gateway runs 2 queries at once and answers 429 `busy`; the UI retries and refreshes layers single-flight.
- Codex (gpt-5.6-terra via `codex exec -s read-only`, delivery mode): its sandbox cannot write files on this machine; it returns FILE/EDIT blocks, applied by a script (scratchpad `apply_delivery.py`, not in the repo). Its shell mangles non-ASCII (so Vietnamese strings and diacritics are written by Claude), and it truncates long answers (check that every promised file arrived; unit tests and some wiring edits were missing twice). Verify its test expectations by hand: three fixture expectations were wrong.
- npm: `geotiff@3.0.5` added to web/ (COG reading in the browser, range requests). Install note per the "document installs" rule.
