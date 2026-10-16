# REPORT — GeoFabric Real Data Integration (Vietnam Demo Data Pack)

Date 2026-10-16 · Branch `main` · Commit: see the last line · Coder: Codex (`gpt-5.6-terra`, delivery mode) and Claude where stated · Reviewer: Claude

Written for: the Principal Architect and the data owner. It states what was built, what was measured, who wrote what, and what is still open.

## 1. Executive Summary

GeoFabric now opens on a real GIS workspace of Ho Chi Minh City: **15.4 million building footprints, 453,550 road segments, 449,849 places, 325 administrative units, 13,277 land-use polygons and a Copernicus terrain model**, over a real basemap. Every vector layer is a normalized GeoParquet file described by a manifest with a pinned release, licence, exact attribution, and sealed checksums; the files are rebuilt by a script and verified against those checksums. Five demo workflows run through PlanGraph, the planner and the executor, and two of them (buildings near main roads, buildings per ward) were checked against an independent DuckDB computation and match exactly.

Things you should know before reading on:

- **Codex could not write files on this machine** (its Windows sandbox refuses edits) and a request to run it with full access was blocked by the tool-permission layer. Codex therefore delivered code as text; I applied it with a script and tested it. Its output was incomplete or wrong several times (section 20); I fixed or completed those parts myself and say so.
- **Buildings have no height or floor data**: those fields are 100 % null in this Overture subset. Nothing was invented.
- **Vietnam has no district tier in the data** (2025 reform): the demo counts per ward/commune (phường/xã) and per province.
- **Raster is display only**, outside PlanGraph. **Metre buffers are not possible on lon/lat in the current IR**: the "200 m" demo uses an explicit angular distance with a disclosed error.
- **Planner P0 is unchanged**: the browser time-model median is still 1.37× (gate 1.25×). Nothing here tried to fix it.
- **No PostGIS.**

## 2. Repository State

HEAD before this phase: `8abe0e6`. Commits of this phase: `7562394` (M1), `93682c4` (M2–M7 + gateway catalog), `52bcbb9` (catalog UI, layers, inspector, tooling), `5bfc499` (query templates, raster), `bdab7a7` (demos, selection, tests) and the final commit named at the end. Working tree clean at the end.

## 3. Data Sources

| dataset | provider | licence | release | coverage | size | format | CRS |
|---|---|---|---|---|---|---|---|
| `overture_buildings_vn_south` | Overture Maps Foundation | ODbL (share-alike), per-source credits | 2026-09-23.1 | southern Vietnam [105.0, 8.5, 108.0, 12.0] | 1.45 GB | GeoParquet (raw Overture schema) | OGC:CRS84 |
| `overture_transport_hcm` | Overture (OSM, TomTom) | ODbL | 2026-09-23.1 | HCMC [106.35, 10.35, 107.05, 11.25] | 36.6 MB | GeoParquet, flat | OGC:CRS84 |
| `overture_places_hcm` | Overture (Meta, Microsoft, Foursquare, AllThePlaces, others) | per source: CDLA-Permissive-2.0, Apache-2.0, CC0-1.0 | 2026-09-23.1 | HCMC | 30.2 MB | GeoParquet, flat | OGC:CRS84 |
| `overture_admin_hcm` | Overture (OSM, geoBoundaries, Esri) | ODbL | 2026-09-23.1 | HCMC | 3.3 MB | GeoParquet, flat | OGC:CRS84 |
| `overture_landuse_hcm` | Overture base (OSM, Daylight) | ODbL | 2026-09-23.1 | HCMC | 3.5 MB | GeoParquet, flat | OGC:CRS84 |
| `copernicus_dem_hcm` | ESA / Copernicus (AWS Open Data) | Copernicus DEM licence, free, attribution mandatory | COP-DEM GLO-30 | 4 tiles, 106–108 E, 10–12 N | 163.5 MB | COG (4 files) | EPSG:4326 |

Total on disk: about 1.65 GB (`data/local` 228 MB + buildings 1.45 GB). Under the 10 GB stop line; no approval was needed.

## 4. Buildings

Kept as the raw Overture file (the manifest records it; the manifest `fetch` reproduces it). Measured (`data/quality/overture_buildings_vn_south.json`): 15,453,032 rows (15,452,933 Polygon, 99 MultiPolygon), 0 invalid geometries, 0 null or empty geometries, 0 duplicate ids, 154 row groups. Null ratio: `height`, `num_floors`, `level`, `min_height`, every roof and facade field **100 %**; `subtype` and `class` 99.8 %; `names` 99.9 %. The Explorer inspector therefore shows area, perimeter, source (and name/type when present), never "height: null". Developer mode lists every raw field.

## 5. Transportation

453,550 LineString segments, flat schema `id, name, subtype, class, subclass, surface, max_speed, max_speed_unit, source_dataset, version, geometry, bbox`. Classes: residential 270,280 · service 93,821 · tertiary 25,266 · secondary 18,007 · primary 13,323 · footway 11,431 and more. Null ratios: name 68.5 %, surface 86 %, max_speed 96 %, subclass 92 % (shown as available, never filled). **Lossy by design, written in the manifest**: only the first `road_surface` and the first `speed_limits` rule is kept (segments can carry several rules for sub-ranges); the big nested columns (connectors, routes, destinations, access restrictions, prohibited transitions) are dropped. There is no `oneway` column: Overture encodes it in `access_restrictions`, and deriving it was out of scope. Road hierarchy is drawn (motorway, primary, secondary, tertiary, local, footway dashed).

## 6. Administrative Boundaries

325 rows: 231 wards/communes (`locality`), 84 microhoods, 6 provinces/cities (`region`; 4 land + 2 territorial-water rows), 2 neighbourhoods, 2 country rows. The data follows the 2025 structure (Thành phố Hồ Chí Minh, Tỉnh Đồng Tháp, Tỉnh Tây Ninh, Thành phố Đồng Nai). **There is no district (quận/huyện) level and none is inferred.** The map layer shows provinces, wards and neighbourhoods on land and hides the country outline and water duplicates. A user who needs the pre-2025 district structure needs another source (for example geoBoundaries ADM2); that was not added.

## 7. POI

449,849 points. Categories used by the demo: schools (`elementary_school` 2,532, `preschool` 2,154, `college_university` 1,536, `high_school` 289, `middle_school` 182), `hospital` 2,422, `government_office` 1,439, `market` 46, `park` 751. 5.7 % have no category; `operating_status` is 100 % null. Colours by category; the layer starts at zoom 14 (below that a 6,000-feature cap would show an arbitrary corner, see section 18).

## 8. Raster

Copernicus DEM GLO-30, 4 COG tiles (3600×3600 each, overviews 1800/900/450). The browser reads them with HTTP range requests through the gateway and draws only the 900×900 overview of each tile: elevation ramp plus hillshade, shown as an image source with opacity control. Elevation in the pack: −82 to 1,624 m. **Display only.** Imagery (Sentinel-2) was not added: DEM was chosen as the better-licensed, smaller raster.

## 9. Thematic Layer

Land use from Overture base (13,277 features: grass, pitch, school grounds, park, residential, industrial, farmland, …), drawn by class at 35 % opacity. One polygon (a 347° × 26° "marine national monument") would have covered the screen and is excluded by an explicit extent guard in the preset (documented in the manifest). 15 of the 13,277 features are lines and are not drawn by the fill layer.

## 10. Data Quality

`data/quality/<id>.json` for every vector dataset: rows, geometry types, null/empty/invalid geometry counts (DuckDB spatial `ST_IsValid`, batched), per-column null ratio, duplicate ids, CRS as resolved by the GeoParquet rules, bbox from the data, row groups. Result for all five vector files: **0 null, 0 empty, 0 invalid geometries, 0 duplicate ids**, CRS OGC:CRS84. Geometries were never repaired.

## 11. CRS

GeoParquet rule implemented and tested: no `crs` key → OGC:CRS84; `crs: null` → UNDEFINED (verify fails unless the manifest says so); explicit PROJJSON → its authority:code. OGC:CRS84 and EPSG:4326 are different strings. Every fetcher asserts after writing that the file's resolved CRS is OGC:CRS84.

## 12. Normalization

Overture vectors: select flat columns (`names.primary AS name`, first address/source record, …), keep Overture's row order (it is spatially sorted, which makes row-group pruning work), ZSTD, 10k–50k rows per group, typed geometry so DuckDB writes GeoParquet metadata. Every dropped or derived column and the land-use extent guard are in each manifest's `notes`, generated from the preset description (single source of truth). The DEM tiles are stored unchanged.

## 13. Storage and Reproducibility

`data/manifests` (committed), `data/scripts` (committed), `data/quality` (committed), `data/local` (ignored). Manifest fields: provider, source, **pinned release** (the validator rejects "latest"), licence + exact attribution + redistribution class (**`unclear` fails validation**), coverage, CRS, formats, files with `size_bytes`, `sha256`, `rows`, `id_digest`, and the fetch command. `geo_data.py verify` recomputes all of it; `bootstrap "<region>" --run` fetches what does not verify, seals it and writes the quality report (`--dry-run` shows the commands).

## 14. Catalog UI

The Explorer sidebar is the catalog: groups (Buildings, Transport, Administrative, Places, Raster, Thematic) with friendly names from the manifests (vi/en), geometry type and size, a diacritics-insensitive search box, and per-layer switches: visibility, opacity, zoom to layer, details (provider, licence, attribution, coverage, rows, size, quality). The old size-ladder synthetic files and shuffled variants appear only in Developer mode, under "Benchmark datasets"; Developer details also show ids, files, CRS, row groups, covering.

## 15. Inspector

One generic function maps a dataset group to labelled rows (vi/en), not buildings-only. Buildings: name, area, perimeter, height, floors, type, class, source. Roads: name, class (translated), type, surface, max speed, **length** (measured on the original geometry), source. Wards: name, level, code, area, source. Places: name, type, category (translated), address, locality, status, confidence, source. Land use: name, class, subtype, surface, area, source. Empty values are omitted in Explorer; Developer lists every raw property. A click selects the top-most feature within 5 px for every layer, highlights it (one dedicated "selected" source), and the inspector measures the original geometry, not the tile-clipped one. A value without a translation is shown humanised, never as a key (a test greps for that).

## 16. Demo Workflows

All five live in the Query tab; the result summary appears as a card on the map (so it stays visible when the bottom tab changes).

| | workflow | how it runs | verified |
|---|---|---|---|
| A | buildings in the map view | gateway `features` template | feature count and drawn polygons tested |
| B | buildings within 200 m of main roads | `near` (Buffer + SpatialJoin, server) for the map, `near_count` for the distinct number | 27,362 distinct buildings in a 4 km window: **identical to an independent DuckDB query** |
| C | buildings per ward | `join_count_by` (SpatialJoin + Aggregate + Sort) | top three wards **identical to an independent query** |
| D | schools and hospitals in the view | `features` and `count_by category`, filter `in` | split schools/hospitals tested |
| E | terrain overlay | COG read in the browser | min/max elevation shown; display only |

B, C go to the server because the browser engine does not support Buffer or SpatialJoin (the planner already refuses them). B's distance is an **explicit angular distance** (metres ÷ 111,320): the gateway returns the approximation and the UI says the east–west error is under 2 %. C counts a building crossing a ward boundary in both wards (stated in the UI).

## 17. Performance

Measured by `PACK_PERF=1 npx playwright test pack-perf` (evidence `benchmarks/results/pack-performance.json`; one machine, Chrome 155, gateway debug build with prebuilt DuckDB):

| viewport | buildings: features / engine ms / response→drawn / heap | roads rows / ms | places rows / ms | land use rows / ms |
|---|---|---|---|---|
| small (z16) | 783 / 245 / 416 ms / +12 MB | 1,015 / 214 | 10,650 / 560 | planner → browser (3 MB file) |
| medium (z14) | 20,000 (limit) / 252 / 2.2 s / +66 MB | 14,427 / 901 | 20,000 / 943 | 1,040 / 179 |
| large (z12) | 20,000 (limit) / 259 / 2.3 s / +29 MB | 20,000 / 1,114 | 20,000 / 902 | 5,842 / 549 |

The 3 MB admin and land-use files are chosen for the browser engine by the planner at small viewports (those rows were planned, not executed, by this harness; the UI path executes them). Browser-safety, health and fallback rules were not bypassed.

## 18. Rendering Limits

Per layer a render policy fixes a minimum zoom and a cap (buildings 11/5,000, roads 10/15,000 with class filtering by zoom, places 14/6,000, land use 10/3,000, admin 0/1,000); the active result is capped at 20,000. Below the minimum zoom the layer is not queried and its row says "Zoom in to see". When a cap is hit the map says so. The numbers come from measurement: with an 8,000 cap the first view showed roads cut off at one edge, and places at zoom 12 showed one corner of the city. **A capped layer is the first N rows in file order, not a random or complete sample.**

## 19. Licences, Attribution, Privacy, Offline

`docs/DATA-LICENSES.md` (engineering notes, **not legal advice**) has a table per dataset with the verbatim attribution and the practical consequences (ODbL share-alike applies to derived databases; GeoFabric does not redistribute the files). Attribution reaches the screen in two places: a data credit line on the map for the visible layers (plus the basemap credit), and Settings → "Data sources and licences" (provider, licence link, credit text). Privacy: the data files stay on the GeoFabric server; the basemap server (OpenFreeMap) learns only which map area is viewed, which Settings says in plain words. Offline: if the basemap cannot load, the map keeps working on a plain background (tested).

## 20. Codex / Claude Review Log

| milestone | Codex deliverable | Claude verdict | what Claude changed or completed |
|---|---|---|---|
| M1 manifest framework | one-liner style code, read whole files into memory, tests unrun | REQUEST CHANGES, round 2 delivered a readable version | one fix (DuckDB needs an Arrow table); tests run by Claude: 10/10 |
| M2–M7 fetchers + manifests | fetchers and manifests delivered; bootstrap and tests not | APPROVE with changes | `ST_GeomFromWKB(geometry)` was invented and fails (column already typed); land-use extent guard added; accurate descriptions; bootstrap `--run` and tests completed by Claude after a second incomplete delivery |
| WP-A gateway | multi-dir, `/api/catalog`, raster endpoint, tests | APPROVE | tests 3/3; catalog raster shape differed from the web types (fixed) |
| WP-B1 catalog UI | modules + main.ts edits; **no unit or e2e tests; inspector wiring edit failed; context-click handlers missing; selection layers referenced layers that do not exist; basemap anchor buried layers under roads; place palette used categories that do not exist in the data** | APPROVE after fixes | all of the above fixed, single-flight refresh + busy retry, real category labels, `in` filter in the gateway, tests written by Claude |
| WP-C tooling | tests delivered, bootstrap edits missing, "©" corrupted to "Ac" | APPROVE after fixes | completed by Claude; DATA-LICENSES text repaired |
| WP-D templates | `near`, `join_count_by`, tests with **three wrong expectations**; `near` grouped by geometry, which the IR forbids | APPROVE after changes | redesigned as `near` (pairs) + `near_count`; expectations recomputed by hand from the fixture geometry |
| raster, demos, glue | not delegated (needed the browser and screenshots) | — | written and tested by Claude |

Honest summary: Codex produced useful first drafts of the data scripts and the Rust templates, but needed a reviewer to run everything; about half of the web integration and all the raster/demos work was written by Claude. Codex's shell mangles non-ASCII (Vietnamese text and symbols were written by Claude), and its answers are truncated when long.

## 21. Tests

| suite | result |
|---|---|
| Python tooling (`data/scripts/tests`) | 15 passed |
| Rust workspace | 163 passed (7 new: catalog 3, templates 4) |
| TypeScript unit | 68 passed (focus, raster ramp/hillshade/tiles, demos, catalog/i18n parity, pipeline keys) |
| Playwright Chrome | 58 passed (smoke, explorer, engines, planner, map, data-pack, screenshots) |
| Playwright Edge | 56 passed |
| dependency boundaries | OK |

`data-pack.spec.ts` covers: catalog groups and friendly names (no raw ids in Explorer), roads/places/admin drawing from real data, minimum-zoom message, type-specific inspector rows after real clicks, opacity restyle without a new request, switch-off removes the layer, search, Settings credits, the five demos, English parity, Developer benchmark group. Tests that need the pack skip with an explanation when it is absent.

## 22. Screenshots

`docs/demo-buildings.png`, `demo-transportation.png`, `demo-boundaries.png`, `demo-poi.png`, `demo-multi-layer.png`, `demo-raster.png` (real basemap, real data), plus the refreshed `docs/ui-final-*.png`.

## 23. Architecture Deviations

1. Raster is shown outside PlanGraph (no raster operator, no planner involvement). Decision needed before raster analytics.
2. Buffer on lon/lat uses an angular distance; the IR has no Transform and Buffer accepts metres only on projected CRSs. A real metre buffer needs a design for projected copies or a Transform node.
3. `near` returns one row per (feature, road) pair because `Aggregate` drops geometry and cannot group by it; `near_count` gives the distinct number. A semi-join in the IR would remove the duplicates (RFC needed).
4. The gateway reads manifests on each catalog request (no caching) and now accepts several `--data` directories and the filter op `in`.
5. The "Dự án"/"Profile" buttons of the early sketch were not added (nothing behind them).

## 24. Remaining Risks

- ODbL share-alike on derived databases if the files are ever redistributed; places have mixed per-source licences. Needs the data owner's legal review.
- The basemap is an external service (graceful fallback exists).
- Capped layers show a file-order sample, not a spatially uniform one.
- Admin: no district level; wards only.
- Buildings carry no height/floors; nothing derived.
- Imagery (Sentinel-2) not done; thematic layer is land use only.
- Gateway debug build timings; release numbers not measured.
- First view of a dataset needs two queries (wide, then focused).

## 25. Planner P0 Status

**Unchanged and outstanding:** the browser time-model median 1.37× against the 1.25× gate (REPORT-GEOFABRIC-PLANNER-HARDENING-II). This phase did not touch the planner, estimator or cost model; the new templates run on the server and were not used to refit anything.

## 26. PostGIS Readiness

Not started and not recommended before the Principal Architect reviews the planner round. This phase adds evidence that real multi-source plans (Buffer, SpatialJoin, Aggregate) execute correctly on DuckDB and a data pack that a second engine could be compared against.

## 27. Recommendation

Accept the data pack and the demo as a product demo of the current architecture. Before the next phase: decide the raster and buffer-units questions (section 23), have the data owner review `docs/DATA-LICENSES.md`, and close the browser time-model P0.

## Commit

Commit titled "Real data integration: dataset search, Settings sources and licences, …" (its hash is in `git log`); earlier commits of this phase are listed in section 2.
