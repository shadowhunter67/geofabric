#!/usr/bin/env python3
"""Planner calibration check: for every crossover case, ask the running gateway what the planner
predicts (no execution) and compare with the measured server / browser times.

Usage:  start the gateway (release, --browser-max-mb 800), then
        python planner_vs_measured.py http://127.0.0.1:8799 results/crossover-run1.json results/crossover-run4.json
Writes results/planner-vs-measured.json and prints a summary. The measurements are the ones the model
was FITTED on (in-sample): this is a consistency check of the implementation, not a held-out validation.
"""
import json
import os
import statistics
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


BASE2 = os.environ.get("BASE2")


def main():
    base = sys.argv[1]
    measured = []
    for fn in sys.argv[2:]:
        measured += json.load(open(fn))["rows"]
    out = []
    for r in measured:
        if r.get("server_ms") is None or "error" in r:
            continue
        crs, ext, cat, focus = DATASETS[r["dataset"]]
        req = {"source": r["dataset"], "kind": r["kind"], "bbox": window(ext, r["fraction"], focus), "crs": crs,
               "limit": r.get("limit"), "group_by": cat if r["kind"] == "count_by" else None}
        p = post(base, "/api/query/plan", req)["physical"]
        cand = {c["engine"]: c for c in p["candidates"]}
        selected_default = None
        if BASE2:
            selected_default = post(BASE2, "/api/query/plan", req)["physical"]["engine"]
        out.append({
            "dataset": r["dataset"], "fraction": r["fraction"], "kind": r["kind"], "limit": r.get("limit"),
            "pred_server": cand["duckdb"]["score_ms"], "pred_browser": cand["duckdb-wasm"]["score_ms"],
            "browser_feasible": cand["duckdb-wasm"]["feasible"], "browser_reject": cand["duckdb-wasm"]["rejected_because"],
            "selected": p["engine"], "selected_default_config": selected_default, "meas_server": r["server_ms"],
            "meas_browser": r.get("browser_ms") if not r.get("browser_error") and not r.get("browser_not_placed") else None,
            "result_rows_est": p["result_rows_estimate"], "rows_actual": r.get("server_rows"),
        })
    both = [o for o in out if o["meas_browser"] is not None and o["pred_browser"] is not None]
    ok = 0
    for o in both:
        actual = "duckdb-wasm" if o["meas_browser"] < o["meas_server"] else "duckdb"
        pred = "duckdb-wasm" if o["pred_browser"] < o["pred_server"] else "duckdb"
        o["actual_fastest"], o["predicted_fastest"] = actual, pred
        ok += actual == pred
    es = [abs(o["pred_server"] - o["meas_server"]) for o in out if o["pred_server"]]
    eb = [abs(o["pred_browser"] - o["meas_browser"]) for o in both]
    summary = {
        "cases_total": len(out), "cases_with_both_measured": len(both),
        "winner_agreement": f"{ok}/{len(both)}",
        "server_median_abs_error_ms": round(statistics.median(es), 1) if es else None,
        "browser_median_abs_error_ms": round(statistics.median(eb), 1) if eb else None,
        "browser_faster_actual": sum(1 for o in both if o["actual_fastest"] == "duckdb-wasm"),
        "browser_faster_predicted": sum(1 for o in both if o["predicted_fastest"] == "duckdb-wasm"),
        "note": "fit data = crossover runs 1 and 4; evaluating on a new run of the same grid is a repeatability check, not a held-out validation",
    }
    if BASE2:
        ok_def = sum(1 for o in both if o["selected_default_config"] == o["actual_fastest"])
        summary["default_config_selection_correct"] = f"{ok_def}/{len(both)} (default memory guard 50 MB: browser infeasible for big files, so a 'miss' there means the guard forbids a winner we could not safely use)"
    json.dump({"summary": summary, "cases": out}, open("results/planner-vs-measured.json", "w"), indent=2)
    print(json.dumps(summary, indent=2))
    for o in both:
        if o["actual_fastest"] != o["predicted_fastest"]:
            print("MISS", o["dataset"][:14], o["fraction"], o["kind"], o["limit"], "pred s/b", round(o["pred_server"]), round(o["pred_browser"]), "meas s/b", round(o["meas_server"]), round(o["meas_browser"]), "rows est/actual", o["result_rows_est"], o["rows_actual"])


if __name__ == "__main__":
    main()
