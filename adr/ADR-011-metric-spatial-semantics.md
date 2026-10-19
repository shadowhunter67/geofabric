# ADR-011 — Metric spatial semantics: a distance in metres is measured in metres

Status: Accepted (2026-10-17), scope: distance predicates and buffers over longitude/latitude data. Evidence below.

## Context

Until this phase the "buildings within 200 m of roads" demo converted metres to degrees (200 / 111,320) and buffered in lon/lat. That is wrong in a quiet way: a degree of longitude is 111.3 km only at the equator (109.3 km at 10.8 N, 1.9 % shorter), a degree of latitude is 110.6 km, so the "200 m" area was an ellipse of about 201 m north–south and 204 m east–west, and it changes with latitude. The UI said "200 m". The engine did not compute 200 m. The IR validator already refused a *metre* buffer on a geographic CRS ("transform to a projected CRS first"), but nothing could do that transform inside a plan, so the gateway worked around the validator with an angular distance.

## Decision

1. **A distance with a linear unit is only evaluated in the plane of a local metric CRS.** Degrees are never converted to metres. The validator refuses
   - `within_distance` with an angular distance (`angular_distance_predicate`),
   - a metric predicate whose CRS is not a local metric CRS (`not_metric_crs`): geographic CRSs, Web Mercator (EPSG:3857: metre units, but its scale error grows with latitude), unknown and undefined CRSs.
2. **CRS changes are explicit plan nodes** (`Transform`: input CRS, output CRS, output column), never hidden in an engine. A transform may add a column (`geometry_metric`) and leave the original geometry in its CRS, so a result is still drawn in lon/lat.
3. **Which metric CRS: the UTM zone of the area the plan touches** (`geo_types::local_metric_crs`). The zone comes from the centroid of the query area (viewport grown by the distance). The function **refuses instead of guessing**:
   - the area is not given in a known geographic CRS;
   - latitude outside 80 S – 84 N (UTM is not defined; polar CRSs are not supported);
   - the worst scale error over the area exceeds 0.5 % (`MAX_SCALE_ERROR`). With `k = 0.9996 (1 + (Δλ cos φ)² / 2)` the limit is `Δλ · cos φ <= 5.9°`: a viewport reaching 5.9° from the zone's central meridian at the equator (more at higher latitude, where meridians converge); a whole UTM zone (±3°) is always accepted.
   The gateway answers 422 `metric_crs_unavailable` with the reason; the UI tells the user to zoom in. Nothing falls back to degrees.
4. **Predicate**: `ST_DWithin(l, r, d)` on the transformed geometries, planar, inclusive. A buffer-and-intersect formulation was measured and rejected: 23 s against 0.06 s on the demo data, and its polygonal buffer approximation changes the answer (27,620 instead of 27,624).
5. **Units are values, not text**: `Distance { value, unit }` in the IR (meter, kilometer, foot, degree); a different unit is a different plan id (no silent normalisation).

## Error bounds

| source of error | size |
|---|---|
| UTM scale factor in the demo area (HCMC, 1.7° from the central meridian of zone 48) | 0.02 % – 0.03 % (measured: 199 m ground distance = 198.95 grid metres) |
| Worst case accepted by the rule | 0.5 % |
| Degree-as-metres approach it replaces | 1.9 % east–west at 10.8 N, growing with latitude |

Distances are *ground distances up to the scale error*. For survey-grade work a geodesic computation would be needed; this is not that.

## Evidence

- `crates/geo-runtime/tests/semi_join.rs`: four points placed by a geodesic computation (pyproj `Geod`, WGS 84) at 199 m and 201 m north and east of a centre point: "within 200 m" returns exactly the two 199 m points; 150 m returns none; 250 m returns all four.
- `crates/geo-ir/tests/semi_join.rs`, `crates/geo-types/src/metric.rs`: validator refusals, zone selection, southern hemisphere (327xx), wide areas refused, polar refused, non-geographic input refused.
- Demo B against an independent oracle (shapely + pyproj, `benchmarks/oracle_near.py`): see the report.

## Consequences

- The `Buffer` node with a degree distance on a geographic CRS remains legal (an angular buffer is a real thing a user may ask for) and still warns; no template produces it.
- A raster or a projected source with its own metric CRS is not handled by the demo templates: they require lon/lat sources.
- Cost: two `Transform` projections per query (only rows that survive the bbox filter). Measured: the 4 km Demo B window takes about 0.3 s.
- Open: a general projected-CRS table beyond UTM (national grids such as VN-2000 / TM-3) would give better accuracy locally; it needs a PROJ-backed CRS catalogue (the type table is still a small built-in one).

## Alternatives rejected

- Geodesic distance (`ST_Distance_Spheroid`): accurate everywhere, but it has no spatial index path in DuckDB at this scale and no join operator; kept as the future route for areas no UTM zone covers.
- Web Mercator metres: wrong by `1/cos(lat)` (2 % at 10.8 N, 40 % at 45 N).
- One hard-coded projected CRS for the world or the country: wrong outside its zone, and "Vietnam" spans three UTM zones.

## Evidence update (2026-10-19)

Independent oracle (`benchmarks/oracle_near.py`: pyarrow + shapely 2 STRtree `dwithin` + pyproj; no DuckDB, no GeoFabric code in the computation), Demo B, 4 km window around (106.70, 10.78), 200 m, 41,150 buildings and 1,279 main-road segments in the window: oracle 27,624 buildings, GeoFabric 27,624, **identical id sets (sha256 equal)**, UTM zone 48N (EPSG:32648). The pre-semantics demo (degrees) said 27,362. Against true WGS 84 geodesic distances, 3 of the 1,955 pairs within 5 % of the limit classify differently (0.15 %), consistent with the 0.02–0.03 % scale error in this area. Timing: 1.6 s end to end (oracle 1.8 s).
