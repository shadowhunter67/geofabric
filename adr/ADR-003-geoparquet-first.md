# ADR-003 — GeoParquet là nguồn dữ liệu đầu tiên

Status: Proposed

Lý do: chuẩn mở, cột, có metadata geometry/bbox, row-group statistics cho pruning, tương thích Arrow/DuckDB. Cần xác minh bbox covering column có trong dataset thật (Overture có; dataset synthetic phải tự ghi).
