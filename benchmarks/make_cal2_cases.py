#!/usr/bin/env python3
"""CAL2: the calibration grid again (the 4 calibration datasets x 4 windows x 3 queries) as harness cases, per network.
Used to (re)fit the browser engine's cost terms after the range-request fix (ADR-009). Never overlaps the held-out cases."""
import json, sys
from make_heldout_cases import window, SYN
DS = {"clustered_10mb_spatial": (SYN, (104.5, 12.5), "EPSG:4326", "category"),
      "clustered_100mb_spatial": (SYN, (104.5, 12.5), "EPSG:4326", "category"),
      "clustered_1gb_spatial": (SYN, (104.5, 12.5), "EPSG:4326", "category"),
      "overture_buildings_vn_south": ([105, 8.5, 108, 12], (106.7, 10.8), "OGC:CRS84", "version")}
nets = sys.argv[1:] or ["LOCAL"]
cases = []
for net in nets:
    for ds, (ext, focus, crs, cat) in DS.items():
        for f in (0.002, 0.01, 0.05, 0.25):
            for kind, limit in (("count_by", None), ("features", 2000), ("features", 50000)):
                cases.append({"id": f"c2{len(cases):03d}", "set": "cal2", "dataset": ds, "crs": crs, "fraction": f, "bbox": window(ext, f, focus),
                              "kind": kind, "limit": limit, "group_by": cat if kind == "count_by" else None, "filter": None,
                              "deterministic": False, "network": net, "browser_state": "warm"})
json.dump({"cases": cases}, open(sys.stdout.fileno(), "w", closefd=False), indent=1)
