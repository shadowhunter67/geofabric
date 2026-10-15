#!/usr/bin/env python3
"""True-cardinality evaluation of the estimator on the held-out cases (no timing, server only).

Each held-out case returns at most LIMIT rows (or a handful of groups), which hides how good the *cardinality* estimate is. This replays every
case's window and filter as an ungrouped COUNT on the server (the true number of matching rows) and compares it with the planner's cardinality
estimate for the same window. The gateway must run like the held-out gateway: calibration loaded, `--no-learn`, histograms ready.

Usage: python eval_cardinality.py http://127.0.0.1:8810 results/heldout-cases.json > results/heldout-cardinality.json
"""
import json
import sys
import urllib.request


def post(base, path, body):
    req = urllib.request.Request(base + path, data=json.dumps(body).encode(), headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=300) as r:
        return json.load(r)


def main():
    base, cases_file = sys.argv[1], sys.argv[2]
    out = {}
    for c in json.load(open(cases_file))["cases"]:
        req = {"source": c["dataset"], "kind": "count", "bbox": c["bbox"], "crs": c["crs"], "filter": c["filter"]}
        req = {k: v for k, v in req.items() if v is not None}
        est = post(base, "/api/query/plan", req)["physical"]["assessment"]["estimate"]
        actual = int(post(base, "/api/query", {**req, "engine": "duckdb", "browser": False})["rows"][0]["count"])
        out[c["id"]] = {
            "dataset": c["dataset"], "kind": c["kind"], "fraction": c["fraction"], "filter": c["filter"], "actual": actual,
            "est": est["cardinality"], "low": est["cardinality_low"], "high": est["cardinality_high"], "quality": est["cardinality_quality"],
            "layout": est["layout"], "basis": est["correction"]["basis"], "spread": est["correction"]["spread"],
        }
    json.dump(out, sys.stdout, indent=1)


if __name__ == "__main__":
    main()
