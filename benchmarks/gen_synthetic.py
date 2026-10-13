#!/usr/bin/env python3
"""Reproducible synthetic GeoParquet generator (explicit seed).

Matrix: distribution {clustered, uniform} x ordering {spatial, shuffled}.
Geometry: small axis-aligned rectangles (Polygon, WKB, EPSG:4326).
Each file has a GeoParquet 1.1-style `bbox` covering struct column so that
Parquet row-group statistics on bbox.xmin/... enable pruning.

Usage:  python gen_synthetic.py --size 10mb --out data
        python gen_synthetic.py --size 100mb --out data
Dependencies: numpy, pyarrow (benchmarks/.venv).
"""
import argparse
import hashlib
import json
import pathlib
import struct

import numpy as np
import pyarrow as pa
import pyarrow.parquet as pq

EXTENT = (100.0, 8.0, 110.0, 24.0)  # xmin, ymin, xmax, ymax (EPSG:4326)
CATEGORIES = ["residential", "commercial", "industrial", "public", "religious", "farm", "school", "other"]
# rows are calibrated from measured file size (zstd) ~= 76 bytes/row (see manifest)
SIZES = {
    "10mb": dict(rows=130_000, row_group=5_000),
    "100mb": dict(rows=1_300_000, row_group=20_000),
    "1gb": dict(rows=13_000_000, row_group=100_000),
}
GENERATOR_VERSION = "1"


def centers(dist: str, n: int, rng: np.random.Generator) -> np.ndarray:
    x0, y0, x1, y1 = EXTENT
    if dist == "uniform":
        return np.column_stack([rng.uniform(x0, x1, n), rng.uniform(y0, y1, n)])
    k = 40
    cx = rng.uniform(x0 + 0.5, x1 - 0.5, k)
    cy = rng.uniform(y0 + 0.5, y1 - 0.5, k)
    sigma = rng.uniform(0.05, 0.4, k)
    weights = rng.dirichlet(np.ones(k) * 0.7)
    which = rng.choice(k, size=n, p=weights)
    xy = np.column_stack([rng.normal(cx[which], sigma[which]), rng.normal(cy[which], sigma[which])])
    xy[:, 0] = np.clip(xy[:, 0], x0, x1)
    xy[:, 1] = np.clip(xy[:, 1], y0, y1)
    return xy


def morton(xy: np.ndarray) -> np.ndarray:
    x0, y0, x1, y1 = EXTENT
    qx = ((xy[:, 0] - x0) / (x1 - x0) * 65535).astype(np.uint32)
    qy = ((xy[:, 1] - y0) / (y1 - y0) * 65535).astype(np.uint32)

    def spread(v):
        v = v.astype(np.uint64)
        v = (v | (v << 16)) & 0x0000FFFF0000FFFF
        v = (v | (v << 8)) & 0x00FF00FF00FF00FF
        v = (v | (v << 4)) & 0x0F0F0F0F0F0F0F0F
        v = (v | (v << 2)) & 0x3333333333333333
        v = (v | (v << 1)) & 0x5555555555555555
        return v

    return spread(qx) | (spread(qy) << np.uint64(1))


WKB_DTYPE = np.dtype([("bo", "u1"), ("type", "<u4"), ("rings", "<u4"), ("pts", "<u4"), ("xy", "<f8", (10,))])


def rect_wkb(xmin, ymin, xmax, ymax) -> pa.Array:
    n = len(xmin)
    rec = np.zeros(n, dtype=WKB_DTYPE)
    rec["bo"], rec["type"], rec["rings"], rec["pts"] = 1, 3, 1, 5
    rec["xy"] = np.column_stack([xmin, ymin, xmax, ymin, xmax, ymax, xmin, ymax, xmin, ymin])
    size = WKB_DTYPE.itemsize
    buf = pa.py_buffer(rec.tobytes())
    return pa.Array.from_buffers(pa.binary(size), n, [None, buf]).cast(pa.binary())


def geo_metadata(bbox) -> bytes:
    return json.dumps(
        {
            "version": "1.1.0",
            "primary_column": "geometry",
            "columns": {
                "geometry": {
                    "encoding": "WKB",
                    "geometry_types": ["Polygon"],
                    "crs": {
                        "type": "GeographicCRS",
                        "name": "WGS 84",
                        "id": {"authority": "EPSG", "code": 4326},
                    },
                    "bbox": list(bbox),
                    "covering": {
                        "bbox": {
                            "xmin": ["bbox", "xmin"],
                            "ymin": ["bbox", "ymin"],
                            "xmax": ["bbox", "xmax"],
                            "ymax": ["bbox", "ymax"],
                        }
                    },
                }
            },
        }
    ).encode()


def generate(dist: str, ordering: str, size: str, seed: int, out: pathlib.Path) -> dict:
    cfg = SIZES[size]
    n = cfg["rows"]
    rng = np.random.default_rng(seed)
    c = centers(dist, n, rng)
    w = rng.uniform(0.0002, 0.002, n)
    h = rng.uniform(0.0002, 0.002, n)
    xmin, ymin, xmax, ymax = c[:, 0] - w, c[:, 1] - h, c[:, 0] + w, c[:, 1] + h
    cat = rng.integers(0, len(CATEGORIES), n)
    height = np.round(rng.gamma(2.0, 6.0, n), 1)
    ids = np.arange(n, dtype=np.int64)

    if ordering == "spatial":
        order = np.argsort(morton(c), kind="stable")
    else:
        order = np.random.default_rng(seed + 1).permutation(n)
    xmin, ymin, xmax, ymax, cat, height, ids = (a[order] for a in (xmin, ymin, xmax, ymax, cat, height, ids))

    bbox_type = pa.struct([(k, pa.float64()) for k in ("xmin", "ymin", "xmax", "ymax")])
    table = pa.table(
        {
            "id": ids,
            "category": pa.array(np.array(CATEGORIES, dtype=object)[cat], type=pa.string()),
            "height": height,
            "geometry": rect_wkb(xmin, ymin, xmax, ymax),
            "bbox": pa.StructArray.from_arrays(
                [pa.array(a) for a in (xmin, ymin, xmax, ymax)], fields=list(bbox_type)
            ),
        }
    )
    data_bbox = (float(xmin.min()), float(ymin.min()), float(xmax.max()), float(ymax.max()))
    table = table.replace_schema_metadata({b"geo": geo_metadata(data_bbox)})
    out.mkdir(parents=True, exist_ok=True)
    name = f"{dist}_{size}_{ordering}"
    path = out / f"{name}.parquet"
    pq.write_table(table, path, row_group_size=cfg["row_group"], compression="zstd", write_statistics=True)
    meta = {
        "name": name,
        "generator_version": GENERATOR_VERSION,
        "seed": seed,
        "geometry_type": "Polygon",
        "crs": "EPSG:4326",
        "distribution": dist,
        "ordering": ordering,
        "row_count": n,
        "row_group_size": cfg["row_group"],
        "row_groups": pq.ParquetFile(path).num_row_groups,
        "bbox": data_bbox,
        "extent": EXTENT,
        "file_size_bytes": path.stat().st_size,
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest() if path.stat().st_size < 300_000_000 else None,
    }
    (out / f"{name}.json").write_text(json.dumps(meta, indent=2))
    return meta


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--size", choices=SIZES, required=True)
    ap.add_argument("--out", default="data")
    ap.add_argument("--seed", type=int, default=42)
    a = ap.parse_args()
    out = pathlib.Path(a.out)
    for dist in ("clustered", "uniform"):
        for ordering in ("spatial", "shuffled"):
            m = generate(dist, ordering, a.size, a.seed, out)
            print(f"{m['name']}: {m['file_size_bytes']/1e6:.1f} MB, rows={m['row_count']}, rg={m['row_groups']}")


if __name__ == "__main__":
    main()
