#!/usr/bin/env python3
"""Turn the CALIBRATION measurements (crossover grid, LOCAL network) into estimator telemetry.

For every measured case it asks a running gateway what the estimator's *raw* (uncorrected) selectivity was for that
exact request and pairs it with the number of rows the server really returned. Output = JSON lines of
`geo_planner::Observation`, loaded by the gateway with `--telemetry` (and `--no-learn` during held-out evaluation, so the
held-out set never teaches the planner).

Usage:  python build_calibration.py http://127.0.0.1:8812 results/crossover-run5.json > results/calibration-set.jsonl
The gateway must run WITHOUT telemetry (so raw_selected is the pure heuristic).
"""
import json
import sys
import urllib.request

DATASETS = {
    "clustered_10mb_spatial": ("EPSG:4326", [100, 8, 110, 24], "category", (104.5, 12.5)),
    "clustered_100mb_spatial": ("EPSG:4326", [100, 8, 110, 24], "category", (104.5, 12.5)),
    "clustered_1gb_spatial": ("EPSG:4326", [100, 8, 110, 24], "category", (104.5, 12.5)),
    "overture_buildings_vn_south": ("OGC:CRS84", [105, 8.5, 108, 12], "version", (106.7, 10.8)),
}


def window(e, f, focus):
    w, h = (e[2] - e[0]) * f ** 0.5, (e[3] - e[1]) * f ** 0.5
    cx = min(max(focus[0], e[0] + w / 2), e[2] - w / 2)
    cy = min(max(focus[1], e[1] + h / 2), e[3] - h / 2)
    return [cx - w / 2, cy - h / 2, cx + w / 2, cy + h / 2]


def post(base, path, body):
    req = urllib.request.Request(base + path, data=json.dumps(body).encode(), headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=120) as r:
        return json.load(r)


def main():
    base = sys.argv[1]
    n = 0
    for fn in sys.argv[2:]:
        for r in json.load(open(fn))["rows"]:
            if "error" in r or r.get("server_ms") is None or r.get("server_rows") is None:
                continue
            crs, ext, cat, focus = DATASETS[r["dataset"]]
            req = {"source": r["dataset"], "kind": r["kind"], "bbox": window(ext, r["fraction"], focus), "crs": crs,
                   "limit": r.get("limit"), "group_by": cat if r["kind"] == "count_by" else None}
            p = post(base, "/api/query/plan", req)["physical"]
            est = p["assessment"]["estimate"]
            grouped = r["kind"] == "count_by"
            limit = 50 if grouped else r.get("limit")
            actual = r["server_rows"]
            obs = {
                "ts_ms": 0, "source": r["dataset"], "kind": r["kind"], "engine": "duckdb",
                "raw_selected": est["raw_selected"], "total_rows": sum(s["total_rows"] for s in p["scans"]),
                "est_rows": p["result_rows_estimate"], "actual_rows": actual, "limit": limit,
                "censored": limit is not None and actual >= limit, "group_by": cat if grouped else None,
                "est_ms": None, "actual_ms": r["server_ms"], "network": "LOCAL", "fallback": False,
            }
            print(json.dumps(obs))
            n += 1
    print(f"{n} observations", file=sys.stderr)


if __name__ == "__main__":
    main()
