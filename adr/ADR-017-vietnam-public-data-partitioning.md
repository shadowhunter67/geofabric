# ADR-017: Vietnam public data partitioning

Status: **Proposed** (2026-11-02). The scheme below is chosen on measurements from the southern-Vietnam buildings; it becomes Accepted only after the pilot proves it end to end (viewport lookup, range reads, cache, startup budget) and the hosting question is settled. Related: ADR-013 (LogicalDataset -> PhysicalSource), ADR-014 (static browser profile), ADR-012 (render sampling).

## Problem

The public demo reads one 0.08 x 0.07 degree window of central Ho Chi Minh City (27.7 MB). It reads as "a test area", not "GeoFabric covers Vietnam". A single national GeoParquet per layer is not an option (several GB; one row-group scan can open most of it), and a country-wide download at startup contradicts the browser-first thesis.

## Evidence (measured, not assumed)

Southern Vietnam buildings (`overture_buildings_vn_south`, Overture 2026-09-23.1): **15,453,032 rows, 1.45 GB, 94 bytes per row**, bbox 105.0-108.0 E, 8.6-12.0 N. Shard-size balance of each candidate scheme (estimated at 94 bytes per row; "shards" = non-empty cells):

| scheme | shards | min / median / P90 / max MB | > 25 MB | > 50 MB |
|---|---|---|---|---|
| uniform grid 0.05 deg | 2,797 | 0.00 / 0.33 / 1.19 / 8.8 | 0 | 0 |
| uniform grid 0.1 deg | 754 | 0.00 / 1.34 / 4.50 / 24.2 | 0 | 0 |
| uniform grid 0.2 deg | 217 | 0.00 / 4.76 / 15.4 / 69.3 | 7 | 1 |
| uniform grid 0.25 deg | 148 | 0.00 / 6.78 / 24.0 / 100.9 | 14 | 2 |
| uniform grid 0.5 deg | 48 | 0.00 / 18.8 / 62.4 / 218.3 | 20 | 11 |
| **adaptive quadtree, leaf <= 25 MB** | **138** | **1.90 / 8.96 / 20.3 / 24.7** | **0** | **0** |
| adaptive quadtree, leaf <= 10 MB | 331 | 0.01 / 4.17 / 7.68 / 9.94 | 0 | 0 |
| province (the only 4 land-province polygons present in the local admin file) | 4 | 158 / 169 / 238 / 264 | 4 | 4 |

Uniform grids cannot be balanced: the same cell is 0 MB in the sea and 100+ MB in the city, so either many near-empty shards (more requests, more manifest) or oversized ones. Province partitioning is rejected on the numbers alone: even with only four provinces every shard is 160-260 MB, over every hosting and range-read budget, and it would need sub-sharding anyway. H3/S2-style cells share the uniform grids' imbalance (density is the cause, not the cell shape); they were analysed, not built.

## Decision (proposed)

1. **Adaptive quadtree**, leaf budget 25 MB (initial experiment, not a hard gate), with a stable quadkey-style id per leaf. Features are assigned to the leaf containing their bbox centre (each feature is in exactly one shard, so a union over shards never duplicates); a shard's `bbox` is its data extent, so features that straddle a cell border are still found.
2. `partition_scheme` (`quadtree-v1`) is versioned independently of the data version (`v2`). A re-partition creates a new scheme and new URLs; published URLs never change.
3. **Manifest hierarchy**: root manifest -> one manifest per dataset -> partition entries (`id, bbox, url, bytes, sha256, format, crs, rows, license, source_release`). The schema, the validation rules (duplicate ids, checksums, commit-pinned https URLs, outlier boxes, row totals) and the viewport lookup are implemented and tested in `web/src/static/partitions.ts`.
4. **Logical vs physical**: the plan names the logical dataset; the static runtime resolves it to the shards of the current view. No IR change. Shard ids are never shown to the user.
5. **Zoom policy before lookup**: a dataset has a `min_zoom`; below it no shard is opened (country and regional views read only the admin layer). Above it, at most a bounded number of shards are fetched (the nearest to the view centre when over budget, reported as partial, never as complete).
6. Rendering overviews (PMTiles for roads/POI/admin) are a separate, render-only path; analytics always run on the GeoParquet shards.

## Hosting constraints found (to be settled before any national push)

* jsDelivr refuses GitHub files over **50 MB** (shards <= 25 MB are fine).
* GitHub recommends repositories under 1 GB and strongly under 5 GB; a hard block applies to single files over 100 MiB. The southern buildings alone are 1.45 GB, so a national buildings layer is likely a multi-GB repository. The national size is **not yet measured**; this is a stop condition for publishing, not for building the pilot.
* If GitHub plus jsDelivr cannot hold the national volume, the alternatives (object storage such as R2, with range support and CORS) need a cost and billing decision from the owner before anything is enabled.

## Consequences / open items

* Pilot first (Hanoi, Ho Chi Minh City, Da Nang, Buon Ma Thuot, a Mekong Delta area), then national generation.
* The national feature counts and sizes per dataset are unknown until a metadata-only count against the Overture release is run.
* The existing admin file has only four land-province polygons; national administrative coverage must be re-extracted before it can be claimed.
