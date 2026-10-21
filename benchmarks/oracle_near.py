#!/usr/bin/env python3
"""Independent oracle for the metric `near` semantics (Demo B: buildings within D metres of main roads).

Independent = no DuckDB, no GeoFabric code in the computation: Parquet is read with pyarrow, geometries are parsed and projected
with shapely 2 + pyproj, and the within-distance decision is taken by shapely's STRtree (`dwithin`). Only the *question* is shared
(window, road classes, distance). The answer is compared with the gateway's `near` and `near_count` responses, and the metric
error of the UTM approach is measured against true WGS 84 geodesic distances (pyproj Geod) on the pairs that are close to the limit.

  python benchmarks/oracle_near.py --gateway http://127.0.0.1:8820 [--bbox x0 y0 x1 y1] [--distance 200]

Needs benchmarks/.venv with pyarrow, shapely, pyproj (documented in CLAUDE.md).
"""
import argparse
import hashlib
import json
import pathlib
import sys
import time
import urllib.request

import numpy as np
import pyarrow.dataset as ds
import pyarrow.compute as pc
import shapely
from pyproj import Geod, Transformer
from shapely.strtree import STRtree

ROOT = pathlib.Path(__file__).resolve().parent.parent
BUILDINGS = ROOT / "benchmarks" / "data" / "overture_buildings_vn_south.parquet"
ROADS = ROOT / "data" / "local" / "overture_transport_hcm.parquet"
MAIN = ["motorway", "trunk", "primary"]


def read(path, bbox, extra=None):
    """Rows whose bbox intersects `bbox` (the same rule as the gateway's bbox filter), as (ids, WKB geometries)."""
    x0, y0, x1, y1 = bbox
    f = (pc.field("bbox", "xmin") <= x1) & (pc.field("bbox", "xmax") >= x0) & (pc.field("bbox", "ymin") <= y1) & (pc.field("bbox", "ymax") >= y0)
    if extra is not None:
        f = f & extra
    t = ds.dataset(str(path), format="parquet").to_table(columns=["id", "geometry"], filter=f)
    return t.column("id").to_pylist(), shapely.from_wkb(t.column("geometry").to_numpy(zero_copy_only=False))


def utm_zone_epsg(lon, lat):
    zone = int((lon + 180) // 6) + 1
    return (32600 if lat >= 0 else 32700) + zone


def project(geoms, transformer):
    return shapely.transform(geoms, lambda xy: np.column_stack(transformer.transform(xy[:, 0], xy[:, 1])))


def post(url, body):
    req = urllib.request.Request(url, data=json.dumps(body).encode(), headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=600) as r:
        return json.load(r)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--gateway", required=True)
    ap.add_argument("--bbox", type=float, nargs=4, default=[106.68, 10.76, 106.72, 10.80])
    ap.add_argument("--distance", type=float, default=200.0)
    ap.add_argument("--out", default=str(ROOT / "benchmarks" / "results" / "oracle-near.json"))
    # other data (PostGIS live verification): the same question on other sources and on several engines
    ap.add_argument("--buildings-file", default=str(BUILDINGS))
    ap.add_argument("--roads-file", default=str(ROADS))
    ap.add_argument("--buildings-source", default="overture_buildings_vn_south", help="source id at the gateway")
    ap.add_argument("--roads-source", default="overture_transport_hcm")
    ap.add_argument("--classes", default=",".join(MAIN), help="road classes that count as main roads")
    ap.add_argument("--engines", default="", help="comma list (e.g. duckdb,postgis): run GeoFabric once per engine and require each to match the oracle")
    a = ap.parse_args()
    main_classes = [c for c in a.classes.split(",") if c]
    x0, y0, x1, y1 = a.bbox
    t0 = time.time()

    # --- the oracle -------------------------------------------------------------------------------------------------
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    epsg = utm_zone_epsg(cx, cy)
    tr = Transformer.from_crs("OGC:CRS84", f"EPSG:{epsg}", always_xy=True)
    inv = Transformer.from_crs(f"EPSG:{epsg}", "OGC:CRS84", always_xy=True)
    # road candidates: the window grown by the distance (conservative degree margins; a prefilter only, the decision is metric)
    mlat = a.distance / 110_574.0 * 1.02
    mlon = a.distance / (111_320.0 * np.cos(np.radians(max(abs(y0), abs(y1)) + 0.01))) * 1.02
    b_ids, b_geo = read(pathlib.Path(a.buildings_file), a.bbox)
    r_ids, r_geo = read(pathlib.Path(a.roads_file), [x0 - mlon, y0 - mlat, x1 + mlon, y1 + mlat], pc.field("class").isin(main_classes))
    bm, rm = project(b_geo, tr), project(r_geo, tr)
    tree = STRtree(rm)
    hits = tree.query(bm, predicate="dwithin", distance=a.distance)  # shape (2, n): input index, tree index
    near_idx = np.unique(hits[0])
    oracle_ids = sorted(b_ids[i] for i in near_idx)

    # --- how wrong is "planar metres in UTM" against true ellipsoidal ground distance? ---------------------------------
    geod = Geod(ellps="WGS84")
    band = [i for i in range(len(bm)) if True]  # candidates close to the limit: nearest-road distance within 5 % of D
    nearest = tree.query_nearest(bm, return_distance=True, all_matches=False)
    idx_b, idx_r = nearest[0]
    dist = nearest[1]
    close = [(int(i), int(j), float(d)) for i, j, d in zip(idx_b, idx_r, dist) if 0.95 * a.distance <= d <= 1.05 * a.distance]
    flips = 0
    for i, j, d in close:
        line = shapely.shortest_line(bm[i], rm[j])
        (px, py), (qx, qy) = line.coords[0], line.coords[-1]
        lon1, lat1 = inv.transform(px, py)
        lon2, lat2 = inv.transform(qx, qy)
        ground = geod.inv(lon1, lat1, lon2, lat2)[2]
        if (d <= a.distance) != (ground <= a.distance):
            flips += 1
    oracle_time = time.time() - t0

    # --- GeoFabric ---------------------------------------------------------------------------------------------------
    with_ = {"source": a.roads_source, "filter": {"column": "class", "op": "in", "values": main_classes}, "distance_m": a.distance}
    base = {"source": a.buildings_source, "bbox": a.bbox, "crs": "OGC:CRS84", "with": with_, "browser": False}
    runs = {}
    for engine in (a.engines.split(",") if a.engines else [None]):
        eb = {**base, **({"engine": engine} if engine else {})}
        t1 = time.time()
        near = post(a.gateway + "/api/query", {**eb, "kind": "near", "limit": 100000})
        count = post(a.gateway + "/api/query", {**eb, "kind": "near_count"})
        ids = sorted(f["properties"]["id"] for f in near["geojson"]["features"])
        runs[engine or "auto"] = {"engine_that_ran": (near.get("physical") or {}).get("engine"), "near_rows": len(ids), "near_count": count["rows"][0]["count"],
                                  "count_engine": (count.get("physical") or {}).get("engine"), "ids_sha256": hashlib.sha256(chr(10).join(map(str, ids)).encode()).hexdigest(),
                                  "seconds": round(time.time() - t1, 2), "ids_equal_oracle": ids == oracle_ids, "distinct_ids": len(set(ids)) == len(ids),
                                  "only_oracle": sorted(set(oracle_ids) - set(ids))[:10], "only_engine": sorted(set(ids) - set(oracle_ids))[:10],
                                  "metric_semantics": near.get("metric_semantics")}
    first = next(iter(runs.values()))
    gf_ids = None
    gf_time = first["seconds"]
    count = {"rows": [{"count": first["near_count"]}]}
    near = {"metric_semantics": first["metric_semantics"]}
    ids_equal = all(r["ids_equal_oracle"] and r["near_count"] == len(oracle_ids) and r["distinct_ids"] for r in runs.values())
    out = {
        "window": a.bbox, "distance_m": a.distance, "utm_epsg": epsg, "buildings_in_window": len(b_ids), "roads_considered": len(r_ids),
        "oracle": {"near_count": len(oracle_ids), "ids_sha256": hashlib.sha256(chr(10).join(map(str, sorted(oracle_ids))).encode()).hexdigest(), "seconds": round(oracle_time, 1)},
        "geofabric_runs": runs,
        "ids_equal": ids_equal,
        "counts_equal": all(r["near_count"] == len(oracle_ids) == r["near_rows"] for r in runs.values()),
        "geodesic_check": {"pairs_within_5pct_of_limit": len(close), "classification_flips_vs_geodesic": flips,
                           "note": "UTM planar distance vs WGS 84 ellipsoidal distance between the nearest points of each pair"},
    }
    pathlib.Path(a.out).write_text(json.dumps(out, indent=1), encoding="utf-8")
    print(json.dumps(out, indent=1))
    sys.exit(0 if out["ids_equal"] and out["counts_equal"] else 1)


if __name__ == "__main__":
    main()
