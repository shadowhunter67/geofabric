# ADR-004 — DuckDB là execution engine đầu tiên

Status: Proposed

Lý do: nhúng được, đọc Parquet/HTTP/S3, extension spatial, xuất Arrow. Rủi ro: extension spatial phải tải lúc chạy (cần mạng hoặc cài sẵn); độ đúng CRS của hàm spatial phải kiểm. Phải đứng sau trait ExecutionEngine (RFC-002).

## Evidence review (hardening, 2026-10-13)

Evidence: works end to end, timeout/cancel via `InterruptHandle` verified, sandbox verified. Open: no second real engine (replaceability only shown with a mock), DuckDB reports no trustworthy scan metrics, build cost (ADR-006). **Stays Proposed.**

## Update 2026-10-13 (multi-engine phase)

A second *real* executor now exists (DuckDB-Wasm in the browser, behind the same contract; the planner chooses between the two, same plan -> same canonical digest). That proves the contract and planner are not DuckDB-server-shaped, **not** that engines of different families are interchangeable: both executors are DuckDB builds. **Stays Proposed.**
