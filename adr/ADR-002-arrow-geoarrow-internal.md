# ADR-002 — Arrow/GeoArrow là biểu diễn nội bộ

Status: Proposed

Dữ liệu giữa các tầng là Arrow RecordBatch; geometry theo GeoArrow nếu engine/crate hỗ trợ, nếu không là WKB kèm metadata CRS. Không dùng GeoJSON trong hot path. Cần đo số lần copy/convert trong benchmark.
