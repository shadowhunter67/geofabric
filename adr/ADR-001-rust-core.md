# ADR-001 — Rust core

Status: Proposed

Lý do: Arrow/Parquet native, build WASM, hiệu năng, an toàn bộ nhớ. Đánh đổi: chi phí biên dịch; DuckDB gắn qua crate `duckdb` (bản bundled biên dịch rất lâu — cần đo). Rust 1.99 đã có trên máy. Xem lại nếu binding DuckDB/Arrow gây cản trở lớn.
