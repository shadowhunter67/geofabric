# RFC-002 — Execution Engine Contract

Status: Proposed

```
trait ExecutionEngine {
  fn id() -> EngineId
  fn capabilities() -> Capabilities   // node/op hỗ trợ, CRS, giới hạn, nguồn đọc được
  fn estimate(plan) -> Estimate       // rows, bytes, memory, thời gian + độ tin cậy
  fn explain(plan) -> ExplainTree     // các bước sẽ chạy, bytes/rows dự kiến
  fn execute(plan, ctx) -> ArrowStream + ExecutionMetrics
}
```

- Engine nhận Spatial IR, tự dịch sang ngôn ngữ riêng (SQL cho DuckDB/PostGIS). Chỉ dịch từ IR đã validate, tham số hoá, không nối chuỗi từ input người dùng.
- Kết quả: Arrow RecordBatch stream (geometry theo GeoArrow/WKB, khai báo CRS).
- `ExecutionMetrics`: elapsed, bytes_read, rows_scanned, rows_returned, row_groups_total/skipped, peak_memory.
- `ctx`: giới hạn thời gian, bộ nhớ, số dòng (chống query đắt).
- Engine đầu tiên: DuckDB (ADR-004). Bài kiểm "engine thứ hai": một `MockEngine` trong test phải chạy cùng plan mà không sửa IR/planner.
