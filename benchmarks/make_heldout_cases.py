#!/usr/bin/env python3
"""Held-out planner evaluation: case list (ADR-010).

Two disjoint sets, deliberately different along every axis the planner could overfit to:

  CALIBRATION (what the cost model was fitted on and what the estimator learns from) - the 'crossover' grid:
     datasets  clustered_{10mb,100mb,1gb}_spatial + the Overture subset
     windows   fractions {0.002, 0.01, 0.05, 0.25} of the extent area, centred on a dense cluster
     queries   count_by(category|version), features limit 2000, features limit 50000
     network   LOCAL (browser and gateway on one host)      browser state: warm
     -> benchmarks/results/crossover-run5.json (+ run1/run4), converted by build_calibration.py

  HELD-OUT (this file) - never used to fit or tune anything:
     file size     30 MB files (a size nothing was fitted on), plus other layouts at 10/100 MB and 1 GB
     distribution  uniform (the calibration set is clustered), other focus points
     layout        spatially shuffled files (row groups overlap: pruning cannot help)
     selectivity   fractions {0.0005, 0.02, 0.1, 0.5}
     cardinality   GROUP BY category (8 groups) and GROUP BY height (hundreds of groups)
     operation     ungrouped COUNT, attribute filter + features, attribute filter + count_by
     browser state warm and cold (instance recycled before the first run)
     network       LOCAL, FAST_LAN, GOOD_BROADBAND, MOBILE, HIGH_LATENCY (the calibration set only has LOCAL)

Usage: python make_heldout_cases.py > results/heldout-cases.json   (seeded: reproducible)
"""
import json
import random

SYN = [100.0, 8.0, 110.0, 24.0]
DATASETS = {
    "clustered_30mb_spatial": (SYN, (104.5, 12.5)),
    "clustered_30mb_shuffled": (SYN, (109.0, 22.0)),
    "uniform_30mb_spatial": (SYN, (105.0, 14.0)),
    "uniform_30mb_shuffled": (SYN, (108.2, 20.5)),
    "uniform_10mb_spatial": (SYN, (103.0, 10.0)),
    "uniform_10mb_shuffled": (SYN, (107.0, 18.0)),
    "clustered_10mb_shuffled": (SYN, (104.5, 12.5)),
    "uniform_100mb_spatial": (SYN, (106.0, 16.0)),
    "uniform_100mb_shuffled": (SYN, (101.5, 9.5)),
    "clustered_100mb_shuffled": (SYN, (108.0, 21.0)),
    "uniform_1gb_spatial": (SYN, (105.5, 15.0)),
}
FRACTIONS = [0.0005, 0.02, 0.1, 0.5]
# (kind, limit, group_by, filter, deterministic)
QUERIES = [
    ("features", 500, None, None, False),
    ("features", 2000, None, None, False),
    ("features", 20000, None, None, False),
    ("count_by", None, "category", None, True),
    ("count_by", None, "height", None, False),
    ("count", None, None, None, True),
    ("features", 2000, None, {"column": "category", "op": "eq", "value": "school"}, False),
    ("count_by", None, "category", {"column": "category", "op": "ne", "value": "other"}, True),
]
NETWORKS = [("LOCAL", 28), ("FAST_LAN", 14), ("GOOD_BROADBAND", 20), ("MOBILE", 20), ("HIGH_LATENCY", 18)]


def window(e, f, focus):
    w, h = (e[2] - e[0]) * f ** 0.5, (e[3] - e[1]) * f ** 0.5
    cx = min(max(focus[0], e[0] + w / 2), e[2] - w / 2)
    cy = min(max(focus[1], e[1] + h / 2), e[3] - h / 2)
    return [round(cx - w / 2, 6), round(cy - h / 2, 6), round(cx + w / 2, 6), round(cy + h / 2, 6)]


def main():
    SEED = int(__import__("os").environ.get("HO_SEED", "20261014"))
    rng = random.Random(SEED)
    names = list(DATASETS)
    cases = []
    n = 0
    for net, count in NETWORKS:
        # round-robin over datasets so every dataset appears under several networks; the 1 GB file only a few times
        for i in range(count):
            ds = names[(i + n) % len(names)]
            if ds.endswith("1gb_spatial") and rng.random() < 0.5:
                ds = names[(i + n + 1) % len(names)]
            f = rng.choice(FRACTIONS)
            kind, limit, group_by, flt, det = rng.choice(QUERIES)
            ext, focus = DATASETS[ds]
            cases.append({
                "id": f"ho{len(cases):03d}", "set": "heldout", "dataset": ds, "crs": "EPSG:4326",
                "fraction": f, "bbox": window(ext, f, focus), "kind": kind, "limit": limit, "group_by": group_by, "filter": flt,
                "deterministic": det, "network": net, "browser_state": "cold" if rng.random() < 0.2 else "warm",
            })
        n += 3
    json.dump({"seed": SEED, "cases": cases}, open(__import__("sys").stdout.fileno(), "w", closefd=False), indent=1)


if __name__ == "__main__":
    main()
