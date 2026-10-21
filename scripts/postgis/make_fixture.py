#!/usr/bin/env python3
"""Deterministic tiny GeoParquet fixture for the PostGIS live gates (Gate 3).

  python scripts/postgis/make_fixture.py <output dir>

Writes `fx_buildings.parquet`, `fx_roads.parquet` and `fx_admin.parquet`: simple known geometries in OGC:CRS84 with the same column
shapes as the real pack (id, class, height, geometry, GeoParquet `bbox` covering struct), including the awkward cases a cross-engine
comparison must survive: NULL class and NULL height values, a group-by that ties on count, buildings placed at known GEODESIC
distances (pyproj Geod, WGS 84) from a road so the 200 m metric predicate has rows on both sides of the limit, a building that
touches a road, and a building straddling two admin units.
"""
import json
import pathlib
import sys

import pyarrow as pa
import pyarrow.parquet as pq
from pyproj import Geod

GEOD = Geod(ellps="WGS84")
X0, Y0 = 106.700, 10.780  # near the centre of Ho Chi Minh City


def wkb_polygon(ring):
    out = bytearray([1]) + (3).to_bytes(4, "little") + (1).to_bytes(4, "little") + len(ring).to_bytes(4, "little")
    for x, y in ring:
        out += __import__("struct").pack("<dd", x, y)
    return bytes(out)


def wkb_line(pts):
    out = bytearray([1]) + (2).to_bytes(4, "little") + len(pts).to_bytes(4, "little")
    for x, y in pts:
        out += __import__("struct").pack("<dd", x, y)
    return bytes(out)


def rect(cx, cy, half_deg=0.00008):
    x0, y0, x1, y1 = cx - half_deg, cy - half_deg, cx + half_deg, cy + half_deg
    return (x0, y0, x1, y1), wkb_polygon([(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)])


def write(path, ids, classes, heights, geoms_bboxes, geometry_type):
    bbox_t = pa.struct([("xmin", pa.float64()), ("ymin", pa.float64()), ("xmax", pa.float64()), ("ymax", pa.float64())])
    table = pa.table({
        "id": pa.array(ids, pa.int64()),
        "class": pa.array(classes, pa.string()),
        "height": pa.array(heights, pa.float64()),
        "geometry": pa.array([g for _, g in geoms_bboxes], pa.binary()),
        "bbox": pa.array([{"xmin": b[0], "ymin": b[1], "xmax": b[2], "ymax": b[3]} for b, _ in geoms_bboxes], bbox_t),
    })
    geo = {"version": "1.1.0", "primary_column": "geometry",
           "columns": {"geometry": {"encoding": "WKB", "geometry_types": [geometry_type], "covering": {"bbox": {
               "xmin": ["bbox", "xmin"], "ymin": ["bbox", "ymin"], "xmax": ["bbox", "xmax"], "ymax": ["bbox", "ymax"]}}}}}
    table = table.replace_schema_metadata({b"geo": json.dumps(geo).encode()})
    pq.write_table(table, path, row_group_size=25, compression="zstd", write_statistics=True)


def main():
    out = pathlib.Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    # --- roads: two east-west lines and one north-south line
    road_lines = [
        ("primary", [(X0 - 0.01, Y0), (X0 + 0.01, Y0)]),
        ("secondary", [(X0 - 0.01, Y0 + 0.0300), (X0 + 0.01, Y0 + 0.0300)]),
        ("primary", [(X0 + 0.0150, Y0 - 0.01), (X0 + 0.0150, Y0 + 0.04)]),
    ]
    roads = []
    for _, pts in road_lines:
        bb = (min(p[0] for p in pts), min(p[1] for p in pts), max(p[0] for p in pts), max(p[1] for p in pts))
        roads.append((bb, wkb_line(pts)))
    write(out / "fx_roads.parquet", [1, 2, 3], [c for c, _ in road_lines], [None, None, None], roads, "LineString")

    # --- buildings: 60 on a grid, north of road 1 at 0, 55, 110, 166, 221, 276 m (rows of 10); then 3 at geodesic 199/201/150 m
    ids, classes, heights, geoms = [], [], [], []
    cls = ["house", "school", "shop", "clinic", None]  # five groups of exactly 12: a count_by tie, one of them NULL
    for i in range(60):
        row, col = divmod(i, 10)
        x = X0 + col * 0.0005
        lon, lat, _ = GEOD.fwd(x, Y0, 0.0, row * 55.2)  # due north by row*55.2 m along the meridian
        ids.append(i + 1)
        classes.append(cls[i % 5])
        heights.append(None if i % 7 == 0 else round((i % 9) * 3.5 + 0.5, 1))
        geoms.append(rect(lon, lat))
    for k, d in enumerate([199.0, 201.0, 150.0]):  # known geodesic distances from the road at Y0 (building centre; the polygon is ~9 m wide)
        lon, lat, _ = GEOD.fwd(X0 + 0.0003 + k * 0.0004, Y0, 0.0, d + 9.0)  # the south edge is ~9 m closer than the centre
        ids.append(61 + k)
        classes.append("probe")
        heights.append(10.0)
        geoms.append(rect(lon, lat))
    # a building that touches (straddles) the road at Y0, and one straddling the border of the two admin units
    ids.append(64); classes.append("touching"); heights.append(5.0); geoms.append(rect(X0 + 0.0020, Y0, 0.00008))
    ids.append(65); classes.append("border"); heights.append(5.0); geoms.append(rect(X0 + 0.0100, Y0 + 0.0010, 0.00012))
    write(out / "fx_buildings.parquet", ids, classes, heights, geoms, "Polygon")

    # --- admin: two adjacent units sharing the meridian X0 + 0.0100
    west = [(X0 - 0.02, Y0 - 0.02), (X0 + 0.0100, Y0 - 0.02), (X0 + 0.0100, Y0 + 0.05), (X0 - 0.02, Y0 + 0.05), (X0 - 0.02, Y0 - 0.02)]
    east = [(X0 + 0.0100, Y0 - 0.02), (X0 + 0.04, Y0 - 0.02), (X0 + 0.04, Y0 + 0.05), (X0 + 0.0100, Y0 + 0.05), (X0 + 0.0100, Y0 - 0.02)]
    adm = []
    for ring in (west, east):
        bb = (min(p[0] for p in ring), min(p[1] for p in ring), max(p[0] for p in ring), max(p[1] for p in ring))
        adm.append((bb, wkb_polygon(ring)))
    write(out / "fx_admin.parquet", [1, 2], ["west", "east"], [None, None], adm, "Polygon")
    print(json.dumps({"dir": str(out), "buildings": len(ids), "roads": 3, "admin": 2}))


if __name__ == "__main__":
    main()
