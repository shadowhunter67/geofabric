# ADR-003 — GeoParquet là nguồn dữ liệu đầu tiên

Status: Accepted (2026-10-13, scope: local GeoParquet files; remote object storage is a separate decision)

Lý do: chuẩn mở, cột, có metadata geometry/bbox, row-group statistics cho pruning, tương thích Arrow/DuckDB. Cần xác minh bbox covering column có trong dataset thật (Overture có; dataset synthetic phải tự ghi).

## CRS semantics (2026-10-13)

The reader follows the GeoParquet spec: absent `crs` → OGC:CRS84, `crs: null` → Undefined, object → parsed. It never silently maps a missing CRS to EPSG:4326. Tests: `crates/geo-source-geoparquet/tests/source.rs` (`crs_*`).

## Evidence review (hardening, 2026-10-13)

Evidence: reader + pruning + CRS semantics tested; 10 MB / 100 MB / 1 GB results validated against an independent oracle. Open: only synthetic rectangles were used; no real-world files (e.g. Overture), no remote object storage, no file without a bbox covering column at scale. **Stays Proposed** until real data is tested (needs owner approval to download).

## Real-data evidence (2026-10-13)

A 1.45 GB Overture Maps buildings subset (15.45 M rows, 154 row groups, Polygon+MultiPolygon, nested `names`/`sources`, CRS key absent -> OGC:CRS84) was read, pruned and queried; 7/7 queries match an independent pyarrow+shapely oracle (counts and hashes, including intersects with real polygons and a spatial join). Findings that shaped the reader: (1) the file declared **no `covering`**, only a physical `bbox` struct column with statistics -> the reader infers the covering by naming convention and flags it `declared: false`; (2) CRS84 semantics work end to end; (3) row-group pruning stays effective on real, spatially sorted data (140 of 154 groups skipped for a 20 km window).
