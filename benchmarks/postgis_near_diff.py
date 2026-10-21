#!/usr/bin/env python3
"""Find and explain the buildings on which PostGIS and the independent oracle disagree for Demo B ("within D metres of a main road").

  python benchmarks/postgis_near_diff.py [--distance 200] [--bbox x0 y0 x1 y1] [--classes motorway,trunk,primary]

The oracle (shapely + pyproj, as benchmarks/oracle_near.py) and PostGIS (ST_DWithin on ST_Transform to the same UTM zone) are run on
the same window; for every building on which they differ, both distances to the nearest main road are printed, together with the
bounding-box/covering numbers that could explain it. Development diagnostic only.
"""
import argparse
import json
import os
import pathlib
import sys

import numpy as np
import pyarrow.compute as pc
import pyarrow.dataset as ds
import shapely
from pyproj import Transformer
from shapely.strtree import STRtree

sys.path.insert(0, str(pathlib.Path(__file__).parent))
import oracle_near as O  # noqa: E402  (reuses read/project/utm_zone_epsg)

from pgq import q  # noqa: E402  (benchmarks/pgq.py: psql helper, read-write account)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--distance", type=float, default=200.0)
    ap.add_argument("--bbox", type=float, nargs=4, default=[106.68, 10.76, 106.72, 10.80])
    ap.add_argument("--classes", default="motorway,trunk,primary")
    a = ap.parse_args()
    classes = a.classes.split(",")
    x0, y0, x1, y1 = a.bbox
    epsg = O.utm_zone_epsg((x0 + x1) / 2, (y0 + y1) / 2)
    tr = Transformer.from_crs("OGC:CRS84", f"EPSG:{epsg}", always_xy=True)
    mlat = a.distance / 110_574.0 * 1.02
    mlon = a.distance / (111_320.0 * np.cos(np.radians(max(abs(y0), abs(y1)) + 0.01))) * 1.02
    b_ids, b_geo = O.read(O.BUILDINGS, a.bbox)
    r_ids, r_geo = O.read(O.ROADS, [x0 - mlon, y0 - mlat, x1 + mlon, y1 + mlat], pc.field("class").isin(classes))
    bm, rm = O.project(b_geo, tr), O.project(r_geo, tr)
    tree = STRtree(rm)
    near = set(b_ids[i] for i in np.unique(tree.query(bm, predicate="dwithin", distance=a.distance)[0]))
    print("oracle:", len(near), "buildings in window:", len(b_ids), "roads:", len(r_ids))

    # PostGIS: the same predicate, set-based, with the same covering windows the gateway template uses
    in_list = ",".join("'" + c + "'" for c in classes)
    sql = f"""
    SELECT b.id FROM geofabric.buildings b
    WHERE ST_MakeEnvelope(b.bbox_xmin,b.bbox_ymin,b.bbox_xmax,b.bbox_ymax,4326) && ST_MakeEnvelope({x0},{y0},{x1},{y1},4326)
      AND b.bbox_xmin <= {x1} AND b.bbox_xmax >= {x0} AND b.bbox_ymin <= {y1} AND b.bbox_ymax >= {y0}
      AND EXISTS (SELECT 1 FROM geofabric.transport r
                  WHERE r.class IN ({in_list})
                    AND r.bbox_xmin <= {x1 + mlon} AND r.bbox_xmax >= {x0 - mlon} AND r.bbox_ymin <= {y1 + mlat} AND r.bbox_ymax >= {y0 - mlat}
                    AND ST_DWithin(ST_Transform(ST_SetSRID(b.geometry,4326),{epsg}), ST_Transform(ST_SetSRID(r.geometry,4326),{epsg}), {a.distance}))
    """
    pg = set(q(sql).split())
    print("postgis:", len(pg))
    only_pg, only_or = sorted(pg - near), sorted(near - pg)
    print("only PostGIS:", len(only_pg), "| only oracle:", len(only_or))
    idx = {b: i for i, b in enumerate(b_ids)}
    for bid in only_pg[:12]:
        i = idx[bid]
        d_or = float(shapely.distance(bm[i], shapely.union_all(rm[tree.query(bm[i].buffer(a.distance * 2))]))) if len(tree.query(bm[i].buffer(a.distance * 2))) else float("nan")
        d_pg = q(f"SELECT min(ST_Distance(ST_Transform(ST_SetSRID(b.geometry,4326),{epsg}), ST_Transform(ST_SetSRID(r.geometry,4326),{epsg}))) FROM geofabric.buildings b, geofabric.transport r WHERE b.id='{bid}' AND r.class IN ({in_list}) AND ST_DWithin(ST_Transform(ST_SetSRID(b.geometry,4326),{epsg}), ST_Transform(ST_SetSRID(r.geometry,4326),{epsg}), {a.distance * 3})").strip()
        gt = q(f"SELECT GeometryType(geometry), ST_NPoints(geometry), ST_IsValid(geometry) FROM geofabric.buildings WHERE id='{bid}'").strip()
        print(f"  {bid}  oracle distance {d_or:.4f} m | postgis distance {d_pg} m | {gt}")
    for bid in only_or[:5]:
        print("  only-oracle", bid)


if __name__ == "__main__":
    main()
