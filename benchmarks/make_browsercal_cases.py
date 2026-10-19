#!/usr/bin/env python3
"""Calibration cases for the BROWSER cost model (planner hardening III). Never used as a held-out set.

The browser time of a query is a sum of parts (plan round trip, engine start, opening the sources, the range requests / the
download, Parquet decode and compute, materialising and converting the result). To fit the parts the cases must make each of them
vary on its own: file size and access strategy (whole file vs range), window size (rows returned), query kind, result size,
warm vs cold engine, and the network (the harness throttles Chrome itself: latency and throughput). Real data is included so the
model also sees wide polygons/lines and real attribute columns, not only the synthetic rectangles.

Seed 20261020 (the held-out set of this phase uses a different seed AND files that no calibration run touched):
  HO_SEED=20261020 python make_browsercal_cases.py > results/browsercal-cases.json
"""
import json
import math
import os
import random
import sys

SEED = int(os.environ.get("HO_SEED", "20261020"))
SYN = [100.0, 8.0, 110.0, 24.0]
HCM = [106.35, 10.35, 107.05, 11.25]
# synthetic files of two earlier independent draws + the seed-42 originals
SYN_FILES = [f"{d}_{s}_{o}{suf}" for suf in os.environ.get("HO_SUFFIXES", "_s2,_s3").split(",") for d in ("clustered", "uniform") for s in ("10mb", "30mb", "100mb") for o in ("spatial", "shuffled")]
EXTRA_1GB = [] if os.environ.get("HO_SUFFIXES") else ["clustered_1gb_shuffled", "uniform_1gb_shuffled"]  # a held-out draw has no 1 GB twin
SYN_QUERIES = [
    ("features", 300, None, None), ("features", 1500, None, None), ("features", 8000, None, None), ("features", 40000, None, None),
    ("count", None, None, None), ("count_by", None, "category", None), ("count_by", None, "height", None),
    ("features", 1500, None, {"column": "category", "op": "eq", "value": "school"}),
    ("count", None, None, {"column": "height", "op": "gt", "value": 20}),
]
# real data pack: (dataset, extent, group column, filter)
PACK = [
    ("overture_transport_hcm", HCM, "class", {"column": "class", "op": "in", "values": ["primary", "secondary", "tertiary"]}),
    ("overture_places_hcm", HCM, "category", {"column": "category", "op": "eq", "value": "restaurant"}),
    ("overture_admin_hcm", HCM, "subtype", None),
    ("overture_landuse_hcm", HCM, "class", None),
]
NETWORKS = [("LOCAL", 44), ("FAST_LAN", 28), ("GOOD_BROADBAND", 32), ("MOBILE", 32), ("HIGH_LATENCY", 28)]


def window(e, f, fx, fy):
    w, h = (e[2] - e[0]) * f ** 0.5, (e[3] - e[1]) * f ** 0.5
    cx = min(max(e[0] + fx * (e[2] - e[0]), e[0] + w / 2), e[2] - w / 2)
    cy = min(max(e[1] + fy * (e[3] - e[1]), e[1] + h / 2), e[3] - h / 2)
    return [round(cx - w / 2, 6), round(cy - h / 2, 6), round(cx + w / 2, 6), round(cy + h / 2, 6)]


def main():
    rng = random.Random(SEED)
    cases = []
    for net, count in NETWORKS:
        for i in range(count):
            f = math.exp(rng.uniform(math.log(2e-4), math.log(0.6)))
            state = "cold" if rng.random() < 0.3 else "warm"
            if i % 4 == 3:  # one case in four on the real data pack
                ds, ext, gcol, flt = PACK[(i // 4) % len(PACK)]
                kind, limit, group_by, f2 = rng.choice([("features", 300, None, None), ("features", 2000, None, None), ("features", 20000, None, None),
                                                        ("count", None, None, None), ("count_by", None, gcol, None), ("features", 2000, None, flt)])
                crs = "OGC:CRS84"
                bbox = window(ext, min(1.0, f * 6), rng.random(), rng.random())
                flt = f2 if f2 is not None else None
            else:
                ds = rng.choice(EXTRA_1GB) if EXTRA_1GB and rng.random() < 0.04 else (rng.choice(SYN_FILES) if os.environ.get("HO_SUFFIXES") else SYN_FILES[(i * 7 + len(cases)) % len(SYN_FILES)])
                kind, limit, group_by, flt = rng.choice(SYN_QUERIES)
                crs = "EPSG:4326"
                bbox = window(SYN, f, rng.random(), rng.random())
            cases.append({
                "id": f"c{len(cases):03d}", "set": "browsercal", "dataset": ds, "crs": crs, "fraction": round(f, 5), "bbox": bbox, "kind": kind,
                "limit": limit, "group_by": group_by, "filter": flt, "deterministic": kind != "features" or limit is None, "network": net, "browser_state": state,
            })
    json.dump({"seed": SEED, "cases": cases}, open(sys.stdout.fileno(), "w", closefd=False), indent=1)


if __name__ == "__main__":
    main()
