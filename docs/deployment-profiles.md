# Deployment profiles

GeoFabric is one architecture with three supported deployment shapes. The plan (PlanGraph, IR 0.3), the planner and the browser executor are the same in all of them; what differs is which engines exist.

| | **A. Static browser** | **B. Local full stack** | **C. Self-hosted server** |
|---|---|---|---|
| UI (MapLibre, Explorer / Developer, vi / en) | yes | yes | yes |
| DuckDB-Wasm (browser engine) | yes | yes | yes |
| Gateway | none (the page answers `/api/*`) | yes, `127.0.0.1` | yes |
| DuckDB server engine | not available | yes | yes |
| PostGIS engine | not available | optional | optional |
| Data | public GeoParquet on a verified range-capable host (HTTP Range); bundled whole-file when no host is named | local files under `--data` | private / external storage |
| Needs | any static host | Rust, Node, Python | your infrastructure |
| Status | **live public demo** | supported, tested | not packaged: no production hosting is provided |

## A. Static browser

```
cd web && npm ci && GF_BASE=/geofabric/ npm run build:static     # web/dist = site + demo/ pack
```

Capabilities are what a browser can do: scan, filter, bbox (through the GeoParquet covering columns), aggregate, project, sort, limit. Anything that needs the server is refused with *"This analysis requires GeoFabric Server."*; metric distance is never converted to degrees. Developer mode lists DuckDB server and PostGIS as "not available in this deployment". Design: [ADR-014](../adr/ADR-014-static-browser-profile.md).

## B. Local full stack

The Gateway serves the UI and the API on `127.0.0.1:8787` (no authentication, never SQL: only structured templates / IR). DuckDB runs next to the files. PostGIS is optional: [postgis.md](postgis.md). The gateway must not be exposed to a network; see [SECURITY.md](../SECURITY.md).

## C. Self-hosted server

The same Gateway behind your own access control, with your storage. There is no packaged production deployment (authentication, TLS, multi-user limits are not implemented): treat it as an architecture direction, not a product.

## Choosing

* Want to try or embed GeoFabric: **A**.
* Need metric distance, spatial joins, big files, PostGIS: **B** locally.
* Anything multi-user: **C** is yours to build; open an issue first.
