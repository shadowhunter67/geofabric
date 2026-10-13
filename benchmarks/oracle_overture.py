#!/usr/bin/env python3
"""Independent oracle for the Overture (real data) benchmark queries.

pyarrow + numpy for attribute/bbox logic (bbox via Overture's own `bbox` columns),
shapely/GEOS for geometry predicates. No DuckDB, no GeoFabric. Output format matches
crates/geo-executor `result_digest` (rows sorted, "|"-joined cells, SHA-256).
Query definitions must match `overture_queries` in crates/geo-bench/src/main.rs.

Usage: python oracle_overture.py [--data data] [--out results]
"""
import argparse
import hashlib
import json
import pathlib

import numpy as np
import pyarrow.parquet as pq
import shapely
from shapely import STRtree

SMALL = (106.60, 10.70, 106.80, 10.90)
LARGE = (105.0, 9.5, 107.5, 11.5)
TRI = shapely.from_wkt("POLYGON((106.62 10.72, 106.78 10.74, 106.70 10.88, 106.62 10.72))")
JA = (106.700, 10.770, 106.710, 10.780)
JB = (106.705, 10.775, 106.715, 10.785)
RET = (106.690, 10.760, 106.730, 10.800)


def sha(rows):
    return hashlib.sha256("\n".join(sorted(rows)).encode()).hexdigest()


def ov(b, w):
    return (b[0] <= w[2]) & (b[2] >= w[0]) & (b[1] <= w[3]) & (b[3] >= w[1])


def oracle_file(path):
    pf = pq.ParquetFile(path)
    n_small = n_large = n_attr = 0
    ver_counts = {}
    ret_rows = []
    cand_tri, cand_a, cand_b = [], [], []  # (row group, local indices)
    for g in range(pf.num_row_groups):
        t = pf.read_row_group(g, columns=["bbox", "version", "id"])
        b = t.column("bbox").combine_chunks()
        xmin, ymin, xmax, ymax = (b.field(n).to_numpy(zero_copy_only=False) for n in ("xmin", "ymin", "xmax", "ymax"))
        box = (xmin, ymin, xmax, ymax)
        ver = t.column("version").to_numpy(zero_copy_only=False)
        ids = t.column("id").to_pylist()
        n_small += int(ov(box, SMALL).sum())
        n_large += int(ov(box, LARGE).sum())
        n_attr += int((ver >= 4).sum())  # NaN-safe: version is non-null int32 in this release
        m = ov(box, SMALL)
        for v in ver[m]:
            ver_counts[int(v)] = ver_counts.get(int(v), 0) + 1
        mr = ov(box, RET)
        for i in np.nonzero(mr)[0]:
            ret_rows.append(f"{ids[i]}|{int(ver[i])}")
        tb = TRI.bounds
        mt = ov(box, tb)
        if mt.any():
            cand_tri.append((g, np.nonzero(mt)[0]))
        ma, mb = ov(box, JA), ov(box, JB)
        if ma.any():
            cand_a.append((g, np.nonzero(ma)[0]))
        if mb.any():
            cand_b.append((g, np.nonzero(mb)[0]))

    def geoms(cands):
        out = []
        for g, idx in cands:
            col = pf.read_row_group(g, columns=["geometry"]).column("geometry").combine_chunks()
            raw = col.to_pylist()
            out.extend(shapely.from_wkb([raw[i] for i in idx]))
        return np.array(out, dtype=object)

    gt = geoms(cand_tri)
    n_int = int(shapely.intersects(gt, TRI).sum()) if len(gt) else 0
    ga, gb = geoms(cand_a), geoms(cand_b)
    join_n = 0
    if len(ga) and len(gb):
        tree = STRtree(gb)
        pairs = tree.query(ga, predicate="intersects")
        join_n = int(pairs.shape[1])
    agg = [f"{k}|{v}" for k, v in ver_counts.items()]
    return {
        "ov_bbox_count_small": {"count": n_small, "hash": sha([str(n_small)])},
        "ov_bbox_count_large": {"count": n_large, "hash": sha([str(n_large)])},
        "ov_attr_filter_fullscan": {"count": n_attr, "hash": sha([str(n_attr)])},
        "ov_intersects_polygon": {"count": n_int, "hash": sha([str(n_int)])},
        "ov_aggregate_version_small": {"count": len(agg), "hash": sha(agg)},
        "ov_spatial_join_small": {"count": join_n, "hash": sha([str(join_n)])},
        "ov_return_rows_small(id,version)": {"count": len(ret_rows), "hash": sha(ret_rows)},
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", default="data")
    ap.add_argument("--out", default="results")
    a = ap.parse_args()
    out = {}
    for p in sorted(pathlib.Path(a.data).glob("overture_*.parquet")):
        out[p.stem] = oracle_file(p)
        print(p.stem, {k: v["count"] for k, v in out[p.stem].items()}, flush=True)
    pathlib.Path(a.out).mkdir(exist_ok=True)
    (pathlib.Path(a.out) / "expected-overture.json").write_text(json.dumps(out, indent=2))


if __name__ == "__main__":
    main()
