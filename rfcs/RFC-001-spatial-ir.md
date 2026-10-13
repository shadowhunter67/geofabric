# RFC-001 — Spatial IR

Status: Proposed

## Mục tiêu

Logical plan độc lập engine, dùng chung cho browser / server / distributed (Constitution 4, 6).

## Node

`Source, Filter, BBoxFilter, Intersects, Buffer, SpatialJoin, Aggregate, Project, Limit, Sort`.
MVP cài: Source, Filter, BBoxFilter, Intersects, Aggregate, Project, Limit, Sort. Buffer/SpatialJoin: định nghĩa kiểu, chưa bắt buộc thực thi.

## Metadata mỗi node

`operation, inputs, schema, geometry_type, crs, units, statistics, lineage, capability_requirements, hints`.
Plan là DAG; mỗi node có `id` = hash(operation + params + input ids + ir_version) để cache và lineage.

## CRS & unit

- `Crs` bắt buộc (EPSG hoặc PROJJSON) trên mọi Source và mọi node có geometry; không có giá trị mặc định.
- `Distance{value, unit}`, `Area{value, unit}` với unit enum tường minh. Không có hàm nhận số trần.
- `Buffer{input, distance: Distance, input_crs, output_crs, error_tolerance}`.
- Validator: thao tác đo lường trên CRS địa lý (độ) → reject, hoặc chèn `Transform` tường minh; không im lặng.
- `BBoxFilter` mang CRS của bbox; khác CRS source → transform tường minh.

## Serialization

JSON có `ir_version`, tag `op`. Không chứa SQL hay tên hàm của engine.

## Câu hỏi mở

Biểu thức Filter (mini-AST vs tập hàm cố định); nơi lưu thống kê nguồn (row-group stats); `Transform` là node riêng hay thuộc tính.
