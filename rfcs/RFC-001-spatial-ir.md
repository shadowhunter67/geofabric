# RFC-001 — Spatial IR

Status: Accepted for IR 0.2; IR 0.3 additions (below) implemented 2026-10-17 (see ADR-011, ADR-012)

## Mục tiêu

Logical plan độc lập engine, dùng chung cho browser / server / distributed (Constitution 4, 6).

## Node

`Source, Filter, BBoxFilter, Intersects, Buffer, Transform, SpatialJoin, SpatialSemiJoin, Aggregate, Project, Limit, Sort`.
MVP cài: Source, Filter, BBoxFilter, Intersects, Aggregate, Project, Limit, Sort. Buffer/SpatialJoin: định nghĩa kiểu, chưa bắt buộc thực thi.

## Metadata mỗi node

`operation, inputs, schema, geometry_type, crs, units, statistics, lineage, capability_requirements, hints`.
**Representation (IR 0.2): DAG / `PlanGraph`**, implemented. IR 0.1 (nested tree) is still readable: `Plan::from_json` migrates it deterministically (equal sub-trees become one shared node); the tree `Node` type remains a builder. Node ids from the builder are content hashes; `plan_id()` hashes content so it never depends on id spelling.

Shape:

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

## IR 0.3 additions (2026-10-17)

Every 0.2 plan is a valid 0.3 plan (`Plan::from_json` reads 0.2 unchanged and stamps 0.3); a 0.2 reader rejects the new nodes by their `op` tag.

**`Transform { input, geometry_column, input_crs, output_crs, output_column }`**: the CRS change is a node. `output_column` equal to `geometry_column` replaces the column; a new name adds a column and keeps the original (so a result can be drawn in its source CRS after a metric predicate). The declared `input_crs` must equal the column's CRS (OGC:CRS84 and EPSG:4326 are different); both CRSs must be known. Server-only (DuckDB `ST_Transform`, `always_xy`); the browser engine declares it unsupported.

**`SpatialSemiJoin { left, right, left_column, right_column, predicate }`**: each LEFT row once if at least one RIGHT row satisfies the predicate (EXISTS semantics, never a row per pair). Output schema = the left schema exactly (no right column, no prefix). Both columns must be geometry columns in the same known CRS. Lowering: `SELECT l.* FROM left l WHERE EXISTS (SELECT 1 FROM right r WHERE <predicate>)` (not a join + DISTINCT: no pair table is built). Null and empty geometries never match.

**`SpatialPredicate::WithinDistance { distance }`** (also valid on `SpatialJoin`): planar distance in the plane of the shared CRS, inclusive. Validator: the CRS must be a local metric CRS (UTM zone, `Crs::is_local_metric`; Web Mercator and geographic CRSs are refused: `not_metric_crs`) and the distance must be linear (`angular_distance_predicate`). Units are part of the plan (200 m and 0.2 km are different plan ids).

**`Expr::HashBucket { column, modulus, remainder }`**: `md5(column) mod modulus = remainder`: deterministic, spatially uniform thinning for RENDERING only (ADR-012). Lowered identically in both dialects (golden fixture `features_sampled_for_render`).

Operation kinds added to `OpKind`: `transform`, `spatial_semi_join`.

## Serialization

JSON có `ir_version`, tag `op`. Không chứa SQL hay tên hàm của engine.

## Câu hỏi mở

Biểu thức Filter (mini-AST vs tập hàm cố định); nơi lưu thống kê nguồn (row-group stats). *(Đã quyết 2026-10-17: `Transform` là node riêng, xem trên.)* Còn mở: bảng CRS ngoài UTM (lưới quốc gia như VN-2000) cần danh mục CRS dựa trên PROJ; semi-join với predicate khác `Intersects`/`WithinDistance`; anti-join.
