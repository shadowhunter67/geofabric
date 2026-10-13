# ADR-004 — DuckDB là execution engine đầu tiên

Status: Proposed

Lý do: nhúng được, đọc Parquet/HTTP/S3, extension spatial, xuất Arrow. Rủi ro: extension spatial phải tải lúc chạy (cần mạng hoặc cài sẵn); độ đúng CRS của hàm spatial phải kiểm. Phải đứng sau trait ExecutionEngine (RFC-002).
