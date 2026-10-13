# GEOFABRIC

Dự án kết nối và sử dụng các thành phần liên quan đến GIS (nguồn dữ liệu, dịch vụ bản đồ, xử lý không gian). Phạm vi cụ thể do user bổ sung dần — **đang ở giai đoạn dựng khung (2026-10-13)**, chưa chọn stack.

## Rules bắt buộc

- Không đọc/in file `.env` (xem rules ở CLAUDE.md gốc workspace).
- Luật kiến trúc: đọc `CONSTITUTION.md` và `AGENTS.md` trước khi sửa code. Đổi IR/CRS/unit/planner/API = cần RFC/ADR.
- Cài thư viện/build nặng (DuckDB bundled, dataset lớn, benchmark GB) phải hỏi user trước và ghi vào đây.
- Dữ liệu GIS lớn (shapefile, GeoTIFF, GPKG, tile) để trong `data/` — đã gitignore, không commit.

## Stack & môi trường

- Hướng (ADR đều Proposed): Rust core, GeoParquet, DuckDB, Arrow/GeoArrow, MapLibre + TypeScript. Máy có sẵn: rustc 1.99, node 24, python 3.12; chưa có docker/duckdb CLI.

## Lệnh thường dùng

- Chưa có.

## Kiến trúc (tóm tắt)

- `src/` — mã nguồn (trống).
- `data/` — dữ liệu GIS cục bộ, không commit.
- `docs/claude/` — lịch sử & quyết định thiết kế.

## Bẫy đã biết (gotchas)

- Chưa có.

## Tài liệu chi tiết — đọc khi cần

| File | Đọc khi |
|---|---|
| [`CONSTITUTION.md`](CONSTITUTION.md), [`AGENTS.md`](AGENTS.md) | Luôn đọc trước khi sửa code |
| [`docs/vision.md`](docs/vision.md), `rfcs/RFC-001..003`, `adr/ADR-001..005` | Cần hiểu thiết kế IR/engine/planner |
| [`docs/claude/lich-su.md`](docs/claude/lich-su.md) | Cần tra lịch sử sửa lỗi / quyết định thiết kế |

**Quy tắc duy trì:** mục mới/lịch sử ghi vào `docs/claude/`, KHÔNG nối dài CLAUDE.md.
