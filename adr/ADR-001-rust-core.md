# ADR-001 — Rust core

Status: Proposed

Lý do: Arrow/Parquet native, build WASM, hiệu năng, an toàn bộ nhớ. Đánh đổi: chi phí biên dịch; DuckDB gắn qua crate `duckdb` (bản bundled biên dịch rất lâu — cần đo). Rust 1.99 đã có trên máy. Xem lại nếu binding DuckDB/Arrow gây cản trở lớn.

## Evidence review (hardening, 2026-10-13)

Evidence: the whole core, planner, adapter and gateway are Rust and work natively (see the hardening report). Open: the WASM argument is unproven (no wasm build was attempted); the DuckDB bundled build is heavy (60 GB of `target/` after debug+release; MSVC LNK1140 PDB limit with debug info, fixed with `debug = 0`). **Stays Proposed.**
