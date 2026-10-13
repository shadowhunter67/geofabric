# RFC-001 — Spatial IR

Status: Proposed (revised 2026-10-13: CRS variants, tree vs DAG)

## Mục tiêu

Logical plan độc lập engine, dùng chung cho browser / server / distributed (Constitution 4, 6).

## Node

`Source, Filter, BBoxFilter, Intersects, Buffer, SpatialJoin, Aggregate, Project, Limit, Sort`.
MVP cài: Source, Filter, BBoxFilter, Intersects, Aggregate, Project, Limit, Sort. Buffer/SpatialJoin: định nghĩa kiểu, chưa bắt buộc thực thi.

## Metadata mỗi node

`operation, inputs, schema, geometry_type, crs, units, statistics, lineage, capability_requirements, hints`.
**Current representation: tree** (`Node` owns its inputs; a shared sub-plan is duplicated). **Target representation: DAG / `PlanGraph`** — accepted as P1 architecture debt, *not* implemented. Content hashes (`node_id`) already exist so the move to a graph does not change node semantics.

Target (not implemented):

```
PlanGraph { nodes: NodeId -> Node, edges, root: NodeId }
```

Why later: common sub-expressions, shared sources, lineage, cache reuse, Spatial Git, distributed planning.

## CRS & unit

- `Crs` bắt buộc trên mọi Source và mọi node có geometry; không có giá trị mặc định. Variants: `Epsg(u32)`, `OgcCrs84`, `Undefined`, `Unknown(String)`. `OgcCrs84` (lon/lat) is **never** implicitly equal to `Epsg(4326)` (axis order). `Undefined` and `Unknown` are *not known*: CRS-sensitive operations reject them.
- GeoParquet mapping (follows the spec): `crs` key absent → `OgcCrs84`; `"crs": null` → `Undefined`; object/string → parsed (EPSG id → `Epsg`, OGC:CRS84 → `OgcCrs84`, otherwise `Unknown`).
- `Distance{value, unit}`, `Area{value, unit}` với unit enum tường minh. Không có hàm nhận số trần.
- `Buffer{input, distance: Distance, input_crs, output_crs, error_tolerance}`.
- Validator: thao tác đo lường trên CRS địa lý (độ) → reject, hoặc chèn `Transform` tường minh; không im lặng.
- `BBoxFilter` mang CRS của bbox; khác CRS source → transform tường minh.

## Serialization

JSON có `ir_version`, tag `op`. Không chứa SQL hay tên hàm của engine.

## Câu hỏi mở

Biểu thức Filter (mini-AST vs tập hàm cố định); nơi lưu thống kê nguồn (row-group stats); `Transform` là node riêng hay thuộc tính.
