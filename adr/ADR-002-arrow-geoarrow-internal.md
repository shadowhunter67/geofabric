# ADR-002 — Arrow/GeoArrow là biểu diễn nội bộ

Status: Proposed

Dữ liệu giữa các tầng là Arrow RecordBatch; geometry theo GeoArrow nếu engine/crate hỗ trợ, nếu không là WKB kèm metadata CRS. Không dùng GeoJSON trong hot path. Cần đo số lần copy/convert trong benchmark.

## Evidence review (hardening, 2026-10-13)

Evidence: results are Arrow batches whose geometry columns carry `geoarrow.wkb` + CRS (from the IR). Open: encoding is WKB (no GeoArrow-native), not zero-copy (see docs/pipeline.md), copies/conversions not measured. **Stays Proposed.**
