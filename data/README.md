# Vietnam Demo Data Pack

Reproducible dataset metadata, fetch scripts, checksums and quality measurements. The data files themselves are NOT committed
(`data/local/` is ignored): they are rebuilt from public sources by the scripts, pinned to a release, and checked against the
sealed sha256 / row count / id digest of each manifest.

- `manifests/` one JSON manifest per dataset (provider, licence + exact attribution, release, coverage, CRS, files with checksums, fetch command)
- `scripts/` `geo_data.py` (list, validate, verify, report, seal, bootstrap), fetchers (`fetch_overture.py`, `fetch_dem.py`), offline tests
- `quality/` measured quality reports (rows, geometry types, null ratios, invalid geometries, duplicate ids, CRS, bbox)
- `local/` downloaded + normalized data (ignored); the Overture buildings live in `benchmarks/data/` (1.45 GB, same checks)

## Build the pack (repository root; Python with duckdb + pyarrow: `benchmarks/.venv`)

```
benchmarks/.venv/Scripts/python data/scripts/geo_data.py bootstrap "Ho Chi Minh City" --run     # roads, places, boundaries, land use, terrain (~0.2 GB)
benchmarks/.venv/Scripts/python data/scripts/geo_data.py bootstrap "Southern Vietnam" --run     # Overture buildings (1.45 GB, about 15 minutes)
benchmarks/.venv/Scripts/python data/scripts/geo_data.py verify                                  # every sealed file: size, sha256, rows, id digest, CRS
```

Nothing is downloaded silently: a manifest pins its release (Overture `2026-09-23.1`, Copernicus DEM GLO-30), and `bootstrap`
skips datasets whose sealed files already verify. Sources: Overture Maps (public S3, anonymous), Copernicus DEM on AWS Open Data.
Licences and the attribution each dataset requires: `docs/DATA-LICENSES.md`.

## Rules
CRS is never implicit (GeoParquet: no `crs` key = OGC:CRS84, `crs: null` = undefined, otherwise the declared CRS). Geometry is never repaired
silently; every transformation (flat columns, first record kept, land-use extent guard) is written in the manifest `notes`.
