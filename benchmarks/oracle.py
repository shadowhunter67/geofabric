#!/usr/bin/env python3
"""Independent oracle for the benchmark queries.

Independence: this script uses pyarrow + numpy only. It does NOT use DuckDB,
the GeoFabric reader, the IR, the planner or the bbox covering column: geometry
extents are decoded straight from the WKB bytes (fixed 93-byte rectangles
written by gen_synthetic.py). A bug in the Rust executor or in DuckDB's
spatial/parquet code therefore cannot be shared with the expected values.

Output: results/expected-<size>.json  {dataset: {query: {count, hash}}}
Hash = sha256 over the sorted row strings joined by "\\n"; cells joined by "|":
ints decimal, strings raw, geometry as lowercase WKB hex. Count queries have a
single row "<n>". Must stay identical to `row_strings` in crates/geo-bench.

Usage: python oracle.py --size 10mb [--data data] [--out results]
"""
import argparse
import hashlib
import json
import pathlib

import numpy as np
import pyarrow.parquet as pq

WKB_LEN = 93  # 1 + 4 + 4 + 4 + 5*2*8
W1 = (104.0, 12.0, 105.0, 13.0)   # ~0.6 % of extent
W2 = (100.0, 8.0, 105.0, 16.0)    # 25 % of extent
JA = (104.0, 12.0, 104.3, 12.3)
JB = (104.2, 12.2, 104.5, 12.5)
CATS = ["residential", "commercial", "industrial", "public", "religious", "farm", "school", "other"]


def sha(rows):
    return hashlib.sha256("\n".join(sorted(rows)).encode()).hexdigest()


def decode(geom_col):
    """binary array of fixed-size rectangle WKB -> (n,93) uint8, xmin,ymin,xmax,ymax"""
    arr = geom_col.combine_chunks() if hasattr(geom_col, "combine_chunks") else geom_col
    offsets = np.frombuffer(arr.buffers()[1], dtype=np.int32)[arr.offset : arr.offset + len(arr) + 1]
    assert (np.diff(offsets) == WKB_LEN).all(), "unexpected WKB layout"
    data = np.frombuffer(arr.buffers()[2], dtype=np.uint8)[offsets[0] : offsets[-1]].reshape(-1, WKB_LEN)
    assert (data[:, 0] == 1).all() and (data[:, 1] == 3).all(), "expected little-endian polygons"
    xy = data[:, 13:].copy().view("<f8").reshape(-1, 10)
    return data, xy[:, 0], xy[:, 1], xy[:, 2], xy[:, 5]


def overlap(x0, y0, x1, y1, w):
    return (x0 <= w[2]) & (x1 >= w[0]) & (y0 <= w[3]) & (y1 >= w[1])


def pair_count(a, b):
    """a, b: (4, n) arrays of xmin,ymin,xmax,ymax; closed-interval overlap."""
    total = 0
    for i in range(0, a.shape[1], 2000):
        ax0, ay0, ax1, ay1 = (a[k][i : i + 2000, None] for k in range(4))
        total += int(((ax0 <= b[2][None, :]) & (ax1 >= b[0][None, :]) & (ay0 <= b[3][None, :]) & (ay1 >= b[1][None, :])).sum())
    return total


def oracle_file(path):
    pf = pq.ParquetFile(path)
    cnt_small = cnt_large = cnt_attr = 0
    cat_counts = {}
    ret_rows = []
    ja, jb = [], []
    for g in range(pf.num_row_groups):
        t = pf.read_row_group(g, columns=["id", "category", "height", "geometry"])
        data, x0, y0, x1, y1 = decode(t.column("geometry"))
        ids = t.column("id").to_numpy()
        cats = np.array(t.column("category").to_pylist(), dtype=object)
        h = t.column("height").to_numpy()
        m1 = overlap(x0, y0, x1, y1, W1)
        m2 = overlap(x0, y0, x1, y1, W2)
        cnt_small += int(m1.sum())
        cnt_large += int(m2.sum())
        cnt_attr += int(((cats == "school") & (h > 20.0)).sum())
        for c in cats[m1]:
            cat_counts[c] = cat_counts.get(c, 0) + 1
        for i in np.nonzero(m1)[0]:
            ret_rows.append(f"{int(ids[i])}|{data[i].tobytes().hex()}")
        ma = overlap(x0, y0, x1, y1, JA)
        mb = overlap(x0, y0, x1, y1, JB)
        ja.append(np.vstack([x0[ma], y0[ma], x1[ma], y1[ma]]))
        jb.append(np.vstack([x0[mb], y0[mb], x1[mb], y1[mb]]))
    a = np.hstack(ja)
    b = np.hstack(jb)
    join_n = pair_count(a, b)
    agg_rows = [f"{k}|{v}" for k, v in cat_counts.items()]
    return {
        "bbox_count_small(0.6%)": {"count": cnt_small, "hash": sha([str(cnt_small)])},
        "bbox_count_large(25%)": {"count": cnt_large, "hash": sha([str(cnt_large)])},
        "attr_filter_fullscan": {"count": cnt_attr, "hash": sha([str(cnt_attr)])},
        "intersects_count_small": {"count": cnt_small, "hash": sha([str(cnt_small)])},
        "aggregate_by_category_small": {"count": len(agg_rows), "hash": sha(agg_rows)},
        "spatial_join_small": {"count": join_n, "hash": sha([str(join_n)])},
        "return_rows_small(id,geometry)": {"count": len(ret_rows), "hash": sha(ret_rows)},
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--size", required=True)
    ap.add_argument("--data", default="data")
    ap.add_argument("--out", default="results")
    a = ap.parse_args()
    data = pathlib.Path(a.data)
    out = {}
    for dist in ("clustered", "uniform"):
        for ordering in ("spatial", "shuffled"):
            name = f"{dist}_{a.size}_{ordering}"
            p = data / f"{name}.parquet"
            if p.exists():
                out[name] = oracle_file(p)
                print(name, {k: v["count"] for k, v in out[name].items()}, flush=True)
    pathlib.Path(a.out).mkdir(exist_ok=True)
    (pathlib.Path(a.out) / f"expected-{a.size}.json").write_text(json.dumps(out, indent=2))


if __name__ == "__main__":
    main()
