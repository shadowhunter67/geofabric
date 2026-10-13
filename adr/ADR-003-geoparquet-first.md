# ADR-003 — GeoParquet là nguồn dữ liệu đầu tiên

Status: Proposed

Lý do: chuẩn mở, cột, có metadata geometry/bbox, row-group statistics cho pruning, tương thích Arrow/DuckDB. Cần xác minh bbox covering column có trong dataset thật (Overture có; dataset synthetic phải tự ghi).

## CRS semantics (2026-10-13)

The reader follows the GeoParquet spec: absent `crs` → OGC:CRS84, `crs: null` → Undefined, object → parsed. It never silently maps a missing CRS to EPSG:4326. Tests: `crates/geo-source-geoparquet/tests/source.rs` (`crs_*`).

## Evidence review (hardening, 2026-10-13)

Evidence: reader + pruning + CRS semantics tested; 10 MB / 100 MB / 1 GB results validated against an independent oracle. Open: only synthetic rectangles were used; no real-world files (e.g. Overture), no remote object storage, no file without a bbox covering column at scale. **Stays Proposed** until real data is tested (needs owner approval to download).
