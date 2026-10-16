# REPORT — GeoFabric UI Finalization

Date 2026-10-16 · Branch `main` · Commit: see the last line · Author: Engineering Lead (Claude Sonnet 5.5)

No Rust code, planner, estimator, engine or contract was changed this round (`git diff` touches `web/`, docs and the two process-safety files only). The planner-hardening numbers (estimator v3, held-out v5) therefore still stand and were not re-measured.

## 1. Executive Summary

The map now shows what the user asked it to show: **separate building footprints over a real basemap**, with the debug rectangles gone from Explorer. The default dataset is the real Overture subset when present, so the first screen is real buildings beside a real river and real roads. A release-gate test counts distinct feature geometries and fails if the result is a single rectangle. All suites pass on Chrome (42) and Edge (41), the Rust workspace (156) and TypeScript (55) tests pass, and the process-safety rule is in `AGENTS.md` and `CLAUDE.md`.

## 2. Root Cause of Map Render Bug

**What was being rendered.** The result source did hold the real feature geometries (the API returned them; the inspector measured them). What a user saw instead was:

1. the **dataset extent** outline and the **row-group rectangles** (green, dashed, filled), drawn by default in every mode: these are debug layers, and they were the only large shapes on screen;
2. the real features, but at the wrong zoom: on selecting a dataset the map was fitted to the **whole dataset extent** (hundreds of km); a 150 m building is then well under one pixel, so 20 000 result features were effectively invisible;
3. **no basemap**: a plain dark background, so even visible shapes had no meaning.

**Why.** The first version of the map treated "show what the engine read" as part of the product, and fitted the camera to the dataset because that is what is known before the first query.

**What changed.** (a) Debug overlays are separate layers, hidden in Explorer, optional (default off) in Developer, always below the result. (b) After the first query, if a typical feature would be smaller than 4 px at the current zoom, the map jumps to the densest area at a zoom where it is about 12 px and re-runs the query for that view (`web/src/focus.ts`, unit-tested). (c) A real basemap (section 5). I did not reproduce the problem on a "before" build with a failing test; the failing-before claim rests on the screenshot of the old build (`docs/ui-explorer-vi.png` in commit f9d914b) and on the layer-order and rendered-count tests that the old layer setup could not satisfy.

## 3. Map Rendering Pipeline

query → rows with a geometry column → GeoJSON at the API boundary → MapLibre source `result` → layers `res-fill` (Polygon/MultiPolygon), `res-line`, `res-pt` (Point) → selection layers. Nothing derived from a bounding box, covering column, row group or dataset extent is ever put in `result`.

## 4. Layer Order

bottom to top: basemap (water, areas, roads) → `rg-fill`, `rg-line`, `extent-line` (debug, hidden unless Developer toggles them on) → `res-fill`, `res-line`, `res-pt` → `res-sel-fill`, `res-sel` (selected, yellow fill + white outline) → basemap place-name (symbol) layers. Result layers are inserted before the basemap's first symbol layer, so names stay readable above the features. Tested by reading the live style's layer order.

## 5. Basemap

OpenFreeMap vector tiles (OpenStreetMap data), style `dark`, fetched once at start; roads, water, areas and place names. Two findings you should know about:

- **CARTO's free raster tiles now return an "API KEY REQUIRED" watermark.** I first used them, saw the watermark in the screenshots, and replaced them with OpenFreeMap (no key). The old code is gone.
- **It is an external dependency**: the map server learns which map area is viewed, never the data. This is stated in Settings (vi/en). A checkbox turns the basemap off. If the style cannot be fetched (offline, blocked), the map works on a plain background (tested). Tests never use the network for the basemap; only the screenshot test does.

## 6. Result Geometry

Tests (`web/e2e/map.spec.ts`, all pass on Chrome and Edge):

- first view: more than 100 result features, **all geometries distinct**, none larger than 0.1 % of the dataset extent (so none can be an extent or bbox rectangle), more than 20 individually drawn polygons (`queryRenderedFeatures`);
- LIMIT 10: exactly 10 features, 10 distinct geometries, 5–10 drawn;
- LIMIT 100: exactly 100 features and distinct geometries, more than 30 drawn;
- the Overture subset: same checks on real footprints.

## 7. Debug Overlays

Explorer: hidden regardless of any stored setting. Developer: Settings → "Show debug layers", default off; with them on, the result is still drawn (tested). Legend shows the overlay entries only when they are on.

## 8. Inspector

Collapsed until a feature is selected (unchanged). Selecting by real mouse click on a drawn polygon opens it, highlights exactly one geometry (yellow, one rendered feature, tested) and lists friendly labels. Changes this round: empty (null) attributes are hidden in Explorer, booleans read Có/Không, CRS is Developer-only, the table wraps long ids (a `nowrap` rule was cutting UUIDs), the panel is a little wider.

## 9. Explorer Mode

Dataset list: friendly name, geometry type, size. Benchmark variants (shuffled, other draws, odd generated names) are Developer-only; the list is ordered by size: Small, Medium, Large, Extra large, Overture. Execution summary as in the brief, optimisation in plain words, "Chi tiết nâng cao" on demand, no credits, no engine names.

## 10. Developer Mode

Engines, scores, confidence, health, probe, network profile, row groups, covering, IR, provenance, logs, plus the debug-layer toggle. Unchanged otherwise.

## 11. Execution UI

Summary-first (recommended place, reasons, time, results, data processed, memory only when measured), alternative engine with its estimate, optimisation block. Fallback in Explorer now reads "This data could not be processed in the browser. It was switched to server processing." (vi: "Không thể xử lý dữ liệu này trên trình duyệt. Đã chuyển sang xử lý trên máy chủ."); Developer keeps the class and reason.

## 12. Pipeline UI

Unchanged from round II (icon, title, short subtitle per node, last node is where it ran).

## 13. I18n

Same components, only text changes. New keys (loading, drawing, focusing, basemap, overlays, Có/Không) exist in both languages; the parity test passes (one developer-only key added to its allow-list).

## 14. Accessibility

ARIA tab pattern and states, keyboard resize and tab navigation, focus visible, status announced with icon + text; all earlier tests still pass. New: loading states "Đang tải dữ liệu…", "Đang chạy phân tích…", "Đang vẽ kết quả…", "Đang phóng tới khu vực có dữ liệu…" in the status line (also `aria-live`).

## 15. Responsive

Layout test now runs at 1366×768, 1440×900, 1920×1080 (map keeps most of the screen, nothing overflows). Not mobile.

## 16. Browser Tests

| suite | Chrome | Edge |
|---|---|---|
| smoke, explorer, engines, planner, map | **42 passed** | **41 passed** (the screenshot test is Chrome-only) |

## 17. Screenshot Evidence

`docs/ui-final-explorer-vi.png`, `ui-final-explorer-en.png`, `ui-final-developer.png`, `ui-final-feature-selected.png`, `ui-final-execution.png` (1600×900; Overture buildings over the real basemap; vi and en have the same layout).

## 18. Regression

Rust `cargo test --workspace --no-default-features`: 156 passed, 0 failed. `arch_check`: OK. TypeScript unit tests: 55 passed (5 new for the focus logic). Playwright as above; engines suite (server vs browser result digests, fallback) passes. Held-out and planner benchmarks were not re-run: no planner, estimator or cost-model code changed, and the UI change does not alter a measured path (the harness specs were only pinned to the same synthetic start dataset with `&source=` so they behave as before).

## 19. Remaining UX Debt

- Overture has columns with raw English names ("Roof direction", "Min floor") and no translation.
- The bottom panel clips the end of the execution summary at 900 px height (it scrolls).
- Project, Profile and "Chạy" buttons from the baseline sketch were **not** added: there is no project or user feature behind them, and adding controls with nothing behind them is what this project's rules forbid. "Chạy phân tích" and "Trợ giúp" are there.
- The first view on a new dataset needs two queries (a wide one, then the focused one); on the 1.45 GB subset both are fast (about 0.4 s) but that is one machine.
- Labels over the result come from the basemap's own symbol layers; there are no labels of our own.
- Basemap tiles need the internet; offline you get a plain background.

## 20. Remaining Architecture Risks

Unchanged from `REPORT-GEOFABRIC-PLANNER-HARDENING-II.md`: browser time model median 1.37× (gate 1.25×), no independent second device, nothing above ~1 GB, estimator "no context" fallback untested on real data. New: an external basemap service is a runtime dependency (optional, with fallback).

## 21. PostGIS Readiness

**Not started and not recommended yet**: the planner round is still "KEEP HARDENING PLANNER" pending Principal Architect review. This round adds no evidence either way.

## Process safety

`AGENTS.md` and `CLAUDE.md` now forbid name-based process kills (`taskkill /IM node.exe`, `pkill node`, …) and global toolchain changes, and require a free-port check before starting a server.

## Commit

Commit titled "UI finalization: real feature geometry visible (focus on densest area), …" (the report is in it; its hash is in `git log`).
