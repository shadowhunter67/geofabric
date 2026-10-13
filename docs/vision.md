# Vision

> GeoFabric is a distributed spatial runtime for cloud-native geospatial computing.

Không phải: clone QGIS/ArcGIS, map viewer, GeoServer/PostGIS mới, database/tile format mới, AI GIS demo.

## Luận điểm

GIS bị phân mảnh (storage, DB, analytics, tile, frontend...) và geometry bị chuyển đổi nhiều lần (SHP→GeoJSON→WKB→PostGIS→MVT→WebGL). GeoFabric giảm serialization, copy, ETL, hạ tầng thừa và round-trip server.

```
Applications / QGIS / Web / SDK
            │
   GeoFabric: Spatial IR + Planner
            │
   ┌────────┼────────┐
  WASM    DuckDB   PostGIS     (adapter thay thế được)
            │
   GeoParquet / COG / PMTiles → Object Storage
```

- At rest: GeoParquet / COG. In memory: Arrow / GeoArrow. Execution: DuckDB (đầu tiên). Visualization: MapLibre.
- GeoJSON chỉ ở biên.

## MVP (vertical slice)

GeoParquet → Spatial IR → Planner (rule-based) → DuckDB → Arrow → MapLibre.
Operation: bbox, filter, intersects, aggregate (spatial join nếu còn scope).
Ngoài scope: raster analytics, 3D, routing, realtime, AI agent, Sedona, PostGIS editing, K8s, distributed scheduler.

## Tài liệu

[CONSTITUTION](../CONSTITUTION.md) · [AGENTS](../AGENTS.md) · RFC-001/002/003 trong `rfcs/` · ADR-001..005 trong `adr/`.
