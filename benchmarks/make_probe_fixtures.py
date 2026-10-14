#!/usr/bin/env python3
"""Tiny GeoParquet fixtures for the browser runtime probe (ADR-009).

One fixture per reader-compatibility class (`geo_source_geoparquet::ReadClass`):
  probe-plain-wkb.parquet     pyarrow-written WKB polygons + declared bbox covering   (class plain_wkb)
  probe-duckdb-typed.parquet  written by DuckDB >= 1.5 from a slice of the real Overture subset (class duckdb_typed)

The fixtures are committed (a few KB each) under web/public/probe/ so the probe needs no gateway data.
Regenerate:  benchmarks/.venv/Scripts/python benchmarks/make_probe_fixtures.py
The duckdb-typed fixture needs benchmarks/data/overture_buildings_vn_south.parquet (not committed); when it is
missing the existing committed fixture is kept.
"""
import json
import pathlib
import sys

import duckdb
import numpy as np
import pyarrow as pa
import pyarrow.parquet as pq

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from gen_synthetic import rect_wkb, geo_metadata  # noqa: E402

OUT = pathlib.Path(__file__).parent.parent / "web" / "public" / "probe"
OUT.mkdir(parents=True, exist_ok=True)


def plain():
    n = 64
    rng = np.random.default_rng(7)
    cx, cy = rng.uniform(104, 106, n), rng.uniform(10, 12, n)
    w = h = 0.001
    xmin, ymin, xmax, ymax = cx - w, cy - h, cx + w, cy + h
    bbox = pa.StructArray.from_arrays([pa.array(a) for a in (xmin, ymin, xmax, ymax)], names=["xmin", "ymin", "xmax", "ymax"])
    t = pa.table({
        "id": np.arange(n, dtype=np.int64),
        "category": pa.array(["a", "b", "c", "d"] * (n // 4)),
        "geometry": rect_wkb(xmin, ymin, xmax, ymax),
        "bbox": bbox,
    })
    t = t.replace_schema_metadata({b"geo": geo_metadata((float(xmin.min()), float(ymin.min()), float(xmax.max()), float(ymax.max())))})
    pq.write_table(t, OUT / "probe-plain-wkb.parquet", compression="zstd", write_statistics=True)


def typed():
    src = pathlib.Path(__file__).parent / "data" / "overture_buildings_vn_south.parquet"
    if not src.exists():
        print("overture subset missing: keeping the committed duckdb-typed fixture")
        return
    con = duckdb.connect()
    con.execute(
        f"COPY (SELECT id, bbox, geometry FROM read_parquet('{src.as_posix()}') USING SAMPLE 64 ROWS (reservoir, 7)) "
        f"TO '{(OUT / 'probe-duckdb-typed.parquet').as_posix()}' (FORMAT parquet, COMPRESSION zstd)"
    )


if __name__ == "__main__":
    plain()
    typed()
    for p in sorted(OUT.glob("*.parquet")):
        md = pq.ParquetFile(p).metadata
        print(p.name, p.stat().st_size, "bytes", md.num_rows, "rows", "|", md.created_by)
