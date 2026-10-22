# ADR-014 — Static browser deployment profile: GeoFabric without a server

Status: Accepted (2026-10-21).

## Context

The product thesis is a lightweight, browser-first spatial layer where the server is optional. Until now the UI only worked against the Gateway (planner, DuckDB, optional PostGIS). A public demo that required a Gateway would need a server (or a tunnel to a private machine), which contradicts the thesis and exposes infrastructure.

## Decision

A **deployment profile**, not a second application. `vite build --mode static` defines `__GF_PROFILE__ = "static"`; the same UI, the same PlanGraph (IR 0.3), the same browser lowering (`engine/sqlgen.ts`) and the same DuckDB-Wasm executor run unchanged. Only the HTTP boundary is replaced: `src/static/gateway.ts` answers the `/api/*` contract inside the page from files published next to it (`demo/sources.json`, `demo/catalog.json`, the GeoParquet files).

* **Planning.** `src/static/ir.ts` builds the same PlanGraph the Rust gateway builds for the kinds a browser can run (`features`, `count`, `count_by`, with attribute filters and render thinning), with the same limits and tie-breaking. The "planner" has exactly one feasible candidate and says so: no cost is invented (`score_ms`/`cost` are null), confidence is labelled as coming from a single engine and row-group statistics only.
* **Capabilities that are absent are not errors.** DuckDB-server and PostGIS are reported as `not available in this deployment` (Developer: candidate rows, health `unknown` with that reason; `postgis.configured = false`). Explorer says only what the user needs: the alternative is "not available: this demo runs entirely in the browser".
* **Server-only operations are refused, never approximated.** `near`/`near_count` (Transform + SpatialSemiJoin: metric distance), `join_count_by` (spatial join) and `intersects_features` (exact intersection) answer HTTP 422 `server_required` ("This analysis requires GeoFabric Server"). The related demos are shown disabled with that sentence. No degree approximation, no fake server response, no simulated PostGIS. An explicit server-engine hint or declining the browser also yields `server_required`: there is no silent substitution.
* **Runtime probe, health and fallback** are unchanged. `/api/probe` and `/api/report` feed an in-page health record; the single bounded fallback of the full stack does not exist here (there is nothing to fall back to) and a browser failure is shown as a failure.
* **Data.** Overture is ODbL and its releases rotate (only the two newest remain on the public bucket, so a pinned remote URL would break), and the app needs the normalized flat schema. The demo therefore bundles a small derived pack (`data/scripts/make_public_demo.py`: one ~9 km × 8 km window of central Ho Chi Minh City, small row groups, SHA-256 in `pack.json`) offered under the ODbL with the required attributions. The page does not preload them; a layer is fetched when it is first queried. **Two GitHub Pages behaviours found only on the deployed site** (local `vite preview` hid both, which is why the static suite also runs against the deployed URL, `PUBLIC_URL`): (1) the CDN answers `HEAD` with `Range: bytes=0-` using `200` and no `Content-Range`, and DuckDB-Wasm needs `206` to open a file for range reads, so on Pages it cannot range-read at all; (2) it gzips `application/octet-stream` for clients that accept gzip and serves byte ranges of the compressed bytes. The static profile therefore uses the browser engine's whole-file mode: each layer (at most 14 MB) is fetched once, on first use, with `fetch` (gzip is decoded transparently) and registered as an in-memory file. Row-group pruning still happens in DuckDB on the downloaded bytes. A host that answers ranged `HEAD` correctly could use range reads by flipping one constant (`FULL_FETCH_MAX` in `static/gateway.ts`); the plan does not change.
* **Base path.** `GF_BASE=/geofabric/` (the repository name) at build time; every URL is resolved against the page, never the origin root.
* **Verification.** `web/e2e-static/` runs the production bundle under the sub-path with no Gateway and no PostGIS, and the same spec runs against the deployed site (`PUBLIC_URL`). A network audit asserts that nothing private (gateway, LAN, PostgreSQL, `/api/*`) is contacted.

## Consequences

* The public demo costs nothing to operate and needs no secrets. The Gateway code, its tests and the full-stack Playwright suite are untouched.
* The browser plan builder is a TypeScript twin of part of the Rust `build_ir` (one more compiler of "request -> plan"); it covers only the three kinds above and is unit-tested against the real browser lowering. The structural fix (shared lowering, RFC-004) stays out of scope.
* DuckDB-Wasm fetches its `parquet` extension (code) from `extensions.duckdb.org` on first use: a runtime dependency on DuckDB's CDN, documented as an external origin in the privacy notes.

## Not decided here

A CDN-hosted or remote-source data pack, PMTiles/COG layers, a cost model for the browser branch, PostGIS as a remote service.
