#!/usr/bin/env python3
"""Final planner validation cases (seed 20261026, data draw _s6): small, medium and LARGE sources.

Large = above the browser model's calibrated range (128 MiB): 300 MB, 500 MB, 1 GB synthetic files (new draw) and the 1.45 GB
Overture buildings (real file; new windows only). R5c is meant to fire there: every large case records what the cost model
predicted, what the policy chose and what was measured.

  HO_SEED=20261026 python make_final_cases.py > results/final-cases.json
"""
import json
import math
import os
import random
import sys

SEED = int(os.environ.get("HO_SEED", "20261026"))
SYN = [100.0, 8.0, 110.0, 24.0]
HCM = [106.35, 10.35, 107.05, 11.25]
VN_S = [105.0, 8.5, 108.0, 12.0]
SMALL = [f"{d}_{s}_{o}_s6" for d in ("clustered", "uniform") for s in ("10mb", "30mb", "100mb") for o in ("spatial", "shuffled")]
LARGE = ["clustered_300mb_spatial_s6", "clustered_300mb_shuffled_s6", "uniform_300mb_spatial_s6", "uniform_300mb_shuffled_s6",
         "clustered_500mb_spatial_s6", "uniform_500mb_shuffled_s6", "clustered_1gb_spatial_s6", "uniform_1gb_shuffled_s6"]
PACK = [
    ("overture_transport_hcm", HCM, "class", {"column": "class", "op": "in", "values": ["primary", "secondary", "tertiary"]}),
    ("overture_places_hcm", HCM, "category", {"column": "category", "op": "eq", "value": "restaurant"}),
    ("overture_admin_hcm", HCM, "subtype", None),
    ("overture_landuse_hcm", HCM, "class", None),
]
SYN_QUERIES = [
    ("features", 300, None, None), ("features", 1500, None, None), ("features", 8000, None, None),
    ("count", None, None, None), ("count_by", None, "category", None),
    ("features", 1500, None, {"column": "category", "op": "eq", "value": "school"}),
    ("count", None, None, {"column": "height", "op": "gt", "value": 20}),
]
# net -> (cases, share of large sources): slow links get few large cases (a 1 GB range query at 150 ms RTT takes minutes)
NETWORKS = [("LOCAL", 56, 0.34), ("FAST_LAN", 36, 0.30), ("GOOD_BROADBAND", 36, 0.22), ("MOBILE", 36, 0.12), ("HIGH_LATENCY", 36, 0.08)]


def window(e, f, fx, fy):
    w, h = (e[2] - e[0]) * f ** 0.5, (e[3] - e[1]) * f ** 0.5
    cx = min(max(e[0] + fx * (e[2] - e[0]), e[0] + w / 2), e[2] - w / 2)
    cy = min(max(e[1] + fy * (e[3] - e[1]), e[1] + h / 2), e[3] - h / 2)
    return [round(cx - w / 2, 6), round(cy - h / 2, 6), round(cx + w / 2, 6), round(cy + h / 2, 6)]


def main():
    rng = random.Random(SEED)
    cases = []
    for net, count, large_share in NETWORKS:
        for i in range(count):
            f = math.exp(rng.uniform(math.log(2e-4), math.log(0.3)))
            state = "cold" if rng.random() < 0.3 else "warm"
            u = rng.random()
            if u < large_share:
                if rng.random() < 0.25:  # the real 1.45 GB buildings file: new windows
                    ds, ext, crs = "overture_buildings_vn_south", VN_S, "OGC:CRS84"
                    kind, limit, gb, flt = rng.choice([("features", 2000, None, None), ("features", 300, None, None), ("count", None, None, None), ("count_by", None, "subtype", None)])
                    bbox = window(ext, min(0.05, f), rng.random(), rng.random())
                    cases.append({"id": f"f{len(cases):03d}", "set": "final", "dataset": ds, "crs": crs, "fraction": round(f, 5), "bbox": bbox, "kind": kind, "limit": limit, "group_by": gb,
                                  "filter": flt, "deterministic": kind != "features", "network": net, "browser_state": state, "size_class": "large"})
                    continue
                ds = rng.choice(LARGE)
                kind, limit, gb, flt = rng.choice(SYN_QUERIES)
                cases.append({"id": f"f{len(cases):03d}", "set": "final", "dataset": ds, "crs": "EPSG:4326", "fraction": round(f, 5), "bbox": window(SYN, f, rng.random(), rng.random()), "kind": kind,
                              "limit": limit, "group_by": gb, "filter": flt, "deterministic": kind != "features", "network": net, "browser_state": state, "size_class": "large"})
            elif u < large_share + 0.18:
                ds, ext, gcol, flt0 = PACK[rng.randrange(len(PACK))]
                kind, limit, gb, flt = rng.choice([("features", 300, None, None), ("features", 2000, None, None), ("features", 20000, None, None), ("count", None, None, None), ("count_by", None, gcol, None), ("features", 2000, None, flt0)])
                cases.append({"id": f"f{len(cases):03d}", "set": "final", "dataset": ds, "crs": "OGC:CRS84", "fraction": round(f, 5), "bbox": window(HCM, min(1.0, f * 6), rng.random(), rng.random()), "kind": kind,
                              "limit": limit, "group_by": gb, "filter": flt, "deterministic": kind != "features", "network": net, "browser_state": state, "size_class": "pack"})
            else:
                ds = rng.choice(SMALL)
                kind, limit, gb, flt = rng.choice(SYN_QUERIES)
                cases.append({"id": f"f{len(cases):03d}", "set": "final", "dataset": ds, "crs": "EPSG:4326", "fraction": round(f, 5), "bbox": window(SYN, f, rng.random(), rng.random()), "kind": kind,
                              "limit": limit, "group_by": gb, "filter": flt, "deterministic": kind != "features", "network": net, "browser_state": state, "size_class": "small"})
    json.dump({"seed": SEED, "cases": cases}, open(sys.stdout.fileno(), "w", closefd=False), indent=1)


if __name__ == "__main__":
    main()
