#!/usr/bin/env python3
"""Offline calibration set for estimator v3 (server only, no browser).

For every calibration dataset and window it asks a gateway (planner, WITHOUT telemetry) for the estimator's raw selectivity and
layout class, runs an ungrouped COUNT on the server for the true number of matching rows (a cardinality: nothing is censored by
a LIMIT), and writes one `Observation` per window (kind "cardinality"); plus GROUP BY runs for group counts.

Calibration datasets are the seed-42 synthetic files and the Overture subset. The held-out evaluation uses files from a
different draw of the generator (suffix `_s2`) and different windows: nothing here can leak into it.

Usage: python calibrate_estimator.py http://127.0.0.1:8812 > results/calibration-v3.jsonl
"""
import json
import sys
import urllib.request

SYN = [100.0, 8.0, 110.0, 24.0]
OV = [105.0, 8.5, 108.0, 12.0]
SIZES = ["10mb", "30mb", "100mb"]
DATASETS = {f"{d}_{s}_{o}": (SYN, "EPSG:4326") for d in ("clustered", "uniform") for s in SIZES for o in ("spatial", "shuffled")}
DATASETS["clustered_1gb_spatial"] = (SYN, "EPSG:4326")
DATASETS["uniform_1gb_spatial"] = (SYN, "EPSG:4326")
DATASETS["overture_buildings_vn_south"] = (OV, "OGC:CRS84")
FRACTIONS = [0.0003, 0.001, 0.004, 0.015, 0.05, 0.15, 0.4]
FOCI = [(0.25, 0.30), (0.62, 0.55)]
FILTERS = [{"column": "category", "op": "eq", "value": "school"}, {"column": "category", "op": "ne", "value": "other"}, {"column": "height", "op": "gt", "value": 20}]  # relative positions inside the extent


def window(e, f, focus):
    w, h = (e[2] - e[0]) * f ** 0.5, (e[3] - e[1]) * f ** 0.5
    cx = min(max(e[0] + focus[0] * (e[2] - e[0]), e[0] + w / 2), e[2] - w / 2)
    cy = min(max(e[1] + focus[1] * (e[3] - e[1]), e[1] + h / 2), e[3] - h / 2)
    return [cx - w / 2, cy - h / 2, cx + w / 2, cy + h / 2]


def post(base, path, body):
    req = urllib.request.Request(base + path, data=json.dumps(body).encode(), headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=300) as r:
        return json.load(r)


def main():
    base = sys.argv[1]
    only = set(sys.argv[2:])
    avail = {s["id"] for s in json.load(urllib.request.urlopen(base + "/api/sources"))["sources"] if "error" not in s}
    n = 0
    for name, (ext, crs) in DATASETS.items():
        if name not in avail or (only and name not in only):
            continue
        cat = "version" if name.startswith("overture") else "category"
        for f in FRACTIONS:
            for focus in FOCI:
                bbox = window(ext, f, focus)
                req = {"source": name, "kind": "count", "bbox": bbox, "crs": crs}
                plan = post(base, "/api/query/plan", req)["physical"]
                est = plan["assessment"]["estimate"]
                res = post(base, "/api/query", {**req, "engine": "duckdb", "browser": False})
                actual = int(res["rows"][0]["count"])
                print(json.dumps({
                    "ts_ms": 0, "source": name, "kind": "cardinality", "engine": "duckdb", "raw_selected": est["raw_selected"],
                    "total_rows": sum(s["total_rows"] for s in plan["scans"]), "est_rows": est["cardinality"], "actual_rows": actual,
                    "limit": None, "censored": False, "group_by": None, "est_ms": None, "actual_ms": res["timing"]["total_ms"]["value"],
                    "network": "LOCAL", "fallback": False, "layout": est["layout"]}))
                n += 1
        # attribute filters (not for the Overture subset: its columns differ)
        if not name.startswith("overture"):
            for f in (0.004, 0.05, 0.4):
                for flt in FILTERS:
                    req = {"source": name, "kind": "count", "bbox": window(ext, f, FOCI[1]), "crs": crs, "filter": flt}
                    plan = post(base, "/api/query/plan", req)["physical"]
                    est = plan["assessment"]["estimate"]
                    res = post(base, "/api/query", {**req, "engine": "duckdb", "browser": False})
                    print(json.dumps({
                        "ts_ms": 0, "source": name, "kind": "cardinality", "engine": "duckdb", "raw_selected": est["raw_selected"],
                        "total_rows": sum(s["total_rows"] for s in plan["scans"]), "est_rows": est["cardinality"], "actual_rows": int(res["rows"][0]["count"]),
                        "limit": None, "censored": False, "group_by": None, "est_ms": None, "actual_ms": res["timing"]["total_ms"]["value"],
                        "network": "LOCAL", "fallback": False, "layout": est["layout"]}))
                    n += 1
        # group counts: the whole extent, then a window
        for gcol in ([cat] + (["height"] if not name.startswith("overture") else [])):
            req = {"source": name, "kind": "count_by", "group_by": gcol, "bbox": window(ext, 0.4, FOCI[0]), "crs": crs}
            plan = post(base, "/api/query/plan", req)["physical"]
            res = post(base, "/api/query", {**req, "engine": "duckdb", "browser": False})
            actual = int(res["rows_total"])
            print(json.dumps({
                "ts_ms": 0, "source": name, "kind": "count_by", "engine": "duckdb", "raw_selected": plan["assessment"]["estimate"]["raw_selected"],
                "total_rows": sum(s["total_rows"] for s in plan["scans"]), "est_rows": plan["result_rows_estimate"], "actual_rows": actual, "limit": 50,
                "censored": actual >= 50, "group_by": gcol, "est_ms": None, "actual_ms": res["timing"]["total_ms"]["value"],
                "network": "LOCAL", "fallback": False, "layout": plan["assessment"]["estimate"]["layout"]}))
            n += 1
    print(f"{n} observations", file=sys.stderr)


if __name__ == "__main__":
    main()
