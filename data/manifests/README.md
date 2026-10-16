# Manifest reference

Each `<id>.json` describes one reproducible dataset. The manifest `id` is snake_case and must exactly match its filename.

Required fields:

- `title` contains non-empty Vietnamese (`vi`) and English (`en`) strings.
- `group` is one of `base`, `buildings`, `transport`, `admin`, `places`, `raster`, `thematic`, or `benchmark`.
- `source.type` is `overture-s3`, `http`, `manual`, or `generated`; `source.release` must be pinned and is never `latest` or `current`.
- `license.attribution` is the exact display text. A license with `redistribution: "unclear"` is rejected.
- `coverage.bbox` is `[xmin, ymin, xmax, ymax]` in longitude/latitude, ordered and within world bounds.
- `crs` is always explicit. `OGC:CRS84`, `EPSG:4326`, and `UNDEFINED` are distinct strings.
- `format_normalized` is `GeoParquet`, `COG`, or `GeoJSON`.
- `files` lists repository-relative paths with forward slashes. `rows` and `id_digest` are `null` for rasters.

For GeoParquet, `id_digest` is SHA-256 over the UTF-8 `id` values sorted by plain string comparison and joined by LF, with no final LF. A null digest means no `id` column is declared.

Validate all manifests from the repository root:

