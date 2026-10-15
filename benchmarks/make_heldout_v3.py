#!/usr/bin/env python3
"""Held-out set v3 (planner hardening II). Fresh in every way the estimator could have been fitted to:

  * data      : files from an independent draw of the generator (suffix `_s2`: new cluster centres, new rows) plus two 1 GB
                shuffled files no calibration run ever touched; the calibration set (calibrate_estimator.py) uses the seed-42 files
  * windows   : size log-uniform in [3e-4, 0.7] of the extent, position uniform: no window of any earlier set is reused
  * queries   : features (limit 300 / 1500 / 8000 / 40000), count, count_by category|height, attribute filter variants
  * network   : LOCAL, FAST_LAN, GOOD_BROADBAND, MOBILE, HIGH_LATENCY;  browser state: warm / cold

Seed 20261016 (the seeds 20261014 and 20261015 belong to the exploratory and the first held-out run). Usage:
  HO_SEED=20261016 python make_heldout_v3.py > results/heldout-cases.json
"""
import json
import math
import os
import random
import sys

SYN = [100.0, 8.0, 110.0, 24.0]
SEED = int(os.environ.get("HO_SEED", "20261016"))
SUFFIX = os.environ.get("HO_SUFFIX", "_s2")
S2 = [f"{d}_{s}_{o}{SUFFIX}" for d in ("clustered", "uniform") for s in ("10mb", "30mb", "100mb") for o in ("spatial", "shuffled")]
EXTRA = ["clustered_1gb_shuffled", "uniform_1gb_shuffled"]
QUERIES = [
    ("features", 300, None, None, False), ("features", 1500, None, None, False), ("features", 8000, None, None, False), ("features", 40000, None, None, False),
    ("count_by", None, "category", None, True), ("count_by", None, "height", None, False), ("count", None, None, None, True),
    ("features", 1500, None, {"column": "category", "op": "eq", "value": "school"}, False),
    ("count_by", None, "category", {"column": "category", "op": "ne", "value": "other"}, True),
    ("count", None, None, {"column": "height", "op": "gt", "value": 20}, True),
]
NETWORKS = [("LOCAL", 28), ("FAST_LAN", 14), ("GOOD_BROADBAND", 20), ("MOBILE", 20), ("HIGH_LATENCY", 18)]


def window(e, f, fx, fy):
    w, h = (e[2] - e[0]) * f ** 0.5, (e[3] - e[1]) * f ** 0.5
    cx = min(max(e[0] + fx * (e[2] - e[0]), e[0] + w / 2), e[2] - w / 2)
    cy = min(max(e[1] + fy * (e[3] - e[1]), e[1] + h / 2), e[3] - h / 2)
    return [round(cx - w / 2, 6), round(cy - h / 2, 6), round(cx + w / 2, 6), round(cy + h / 2, 6)]


def main():
    rng = random.Random(SEED)
    cases = []
    k = 0
    for net, count in NETWORKS:
        for i in range(count):
            # 1 GB shuffled files only occasionally (the browser cannot run them safely and they are slow to measure)
            ds = rng.choice(EXTRA) if rng.random() < 0.04 else S2[(i + k) % len(S2)]
            f = math.exp(rng.uniform(math.log(3e-4), math.log(0.7)))
            kind, limit, group_by, flt, det = rng.choice(QUERIES)
            cases.append({
                "id": f"h3{len(cases):03d}", "set": "heldout-v3", "dataset": ds, "crs": "EPSG:4326", "fraction": round(f, 5),
                "bbox": window(SYN, f, rng.random(), rng.random()), "kind": kind, "limit": limit, "group_by": group_by, "filter": flt,
                "deterministic": det, "network": net, "browser_state": "cold" if rng.random() < 0.25 else "warm",
            })
        k += 5
    json.dump({"seed": SEED, "cases": cases}, open(sys.stdout.fileno(), "w", closefd=False), indent=1)


if __name__ == "__main__":
    main()
