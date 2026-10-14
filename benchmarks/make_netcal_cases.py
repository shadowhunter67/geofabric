#!/usr/bin/env python3
"""Network calibration cases: the CALIBRATION datasets (never the held-out ones) run in the browser under several
simulated networks, recording the number of Parquet range requests and bytes per query (gateway /api/stats).
These runs fit the browser's network terms (requests per query, effective parallelism); the held-out set is not involved."""
import json, sys
from make_heldout_cases import window, SYN
DS = {"clustered_10mb_spatial": (SYN, (104.5, 12.5), "EPSG:4326", "category"),
      "clustered_100mb_spatial": (SYN, (104.5, 12.5), "EPSG:4326", "category"),
      "overture_buildings_vn_south": ([105, 8.5, 108, 12], (106.7, 10.8), "OGC:CRS84", "version")}
cases = []
for net in ["LOCAL", "GOOD_BROADBAND", "MOBILE", "HIGH_LATENCY"]:
    for ds, (ext, focus, crs, cat) in DS.items():
        for f in (0.002, 0.05):
            for kind, limit in (("count_by", None), ("features", 2000)):
                cases.append({"id": f"nc{len(cases):03d}", "set": "netcal", "dataset": ds, "crs": crs, "fraction": f, "bbox": window(ext, f, focus),
                              "kind": kind, "limit": limit, "group_by": cat if kind == "count_by" else None, "filter": None,
                              "deterministic": False, "network": net, "browser_state": "warm"})
json.dump({"cases": cases}, open(sys.stdout.fileno(), "w", closefd=False), indent=1)
