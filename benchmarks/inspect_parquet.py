#!/usr/bin/env python3
"""Inspect a (Geo)Parquet file's metadata only (no data pages are decoded).

Reports: schema (nested), GeoParquet `geo` key, row/row-group counts, compression,
encodings, statistics availability for top-level and bbox columns, sizes.
Usage: python inspect_parquet.py data/overture_buildings_vn_south.parquet [--json out.json]
"""
import argparse
import collections
import json
import pathlib

import pyarrow.parquet as pq


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("path")
    ap.add_argument("--json")
    a = ap.parse_args()
    p = pathlib.Path(a.path)
    pf = pq.ParquetFile(p)
    md = pf.metadata
    out = {"file": p.name, "size_bytes": p.stat().st_size, "rows": md.num_rows, "row_groups": md.num_row_groups,
           "created_by": md.created_by, "columns_leaf": md.num_columns}
    out["schema"] = [{"name": f.name, "type": str(f.type)[:140]} for f in pf.schema_arrow]
    kv = md.metadata or {}
    geo = kv.get(b"geo")
    out["geo_key_present"] = geo is not None
    if geo:
        g = json.loads(geo)
        col = g["columns"][g["primary_column"]]
        out["geo"] = {
            "version": g.get("version"), "primary_column": g.get("primary_column"), "encoding": col.get("encoding"),
            "geometry_types": col.get("geometry_types"), "crs_key_present": "crs" in col,
            "crs": (json.dumps(col["crs"])[:200] if col.get("crs") else col.get("crs")),
            "bbox": col.get("bbox"), "covering": col.get("covering"),
        }
    comp = collections.Counter()
    enc = collections.Counter()
    stats = collections.Counter()
    rg_rows = []
    rg_bytes = []
    bbox_stats_leaf = {}
    for i in range(md.num_row_groups):
        rg = md.row_group(i)
        rg_rows.append(rg.num_rows)
        rg_bytes.append(rg.total_byte_size)
        for j in range(rg.num_columns):
            c = rg.column(j)
            comp[c.compression] += 1
            enc.update(str(e) for e in c.encodings)
            has = c.is_stats_set
            stats[(c.path_in_schema.split(".")[0], has)] += 1
            if c.path_in_schema.startswith("bbox."):
                bbox_stats_leaf.setdefault(c.path_in_schema, 0)
                bbox_stats_leaf[c.path_in_schema] += int(has)
    out["compression"] = dict(comp)
    out["encodings"] = dict(enc)
    out["row_group_rows"] = {"min": min(rg_rows), "max": max(rg_rows), "mean": sum(rg_rows) / len(rg_rows)}
    out["row_group_uncompressed_bytes"] = {"min": min(rg_bytes), "max": max(rg_bytes)}
    out["statistics_by_top_level_column"] = {k[0]: v for k, v in
                                              ((k, sum(n for kk, n in stats.items() if kk[0] == k[0] and kk[1])) for k in stats)}
    out["bbox_leaf_statistics_row_groups"] = bbox_stats_leaf
    text = json.dumps(out, indent=2, default=str)
    print(text)
    if a.json:
        pathlib.Path(a.json).write_text(text)


if __name__ == "__main__":
    main()
