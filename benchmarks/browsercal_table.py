#!/usr/bin/env python3
"""Join the measured browser runs of a calibration/held-out set with the planner's own view of the same case.

For every case the gateway is asked (`/api/query/plan`, deterministic, nothing executes) what it knows: row groups kept, bytes,
covering bytes, uncompressed geometry/attribute bytes per row, estimated rows, browser access. The measured side comes from the
harness JSON (phases, requests, bytes, times). Output: one flat JSON table (rows) that the fit and the evaluation both read.

  python browsercal_table.py http://127.0.0.1:8830 <prefix> <cases.json> <out.json>
"""
import json
import statistics as st
import sys
import urllib.request

CDP = {"LOCAL": None, "FAST_LAN": (5, 1000e6 / 8, 1000e6 / 8), "GOOD_BROADBAND": (20, 200e6 / 8, 50e6 / 8), "MOBILE": (70, 30e6 / 8, 10e6 / 8), "HIGH_LATENCY": (150, 20e6 / 8, 5e6 / 8)}


def post(base, path, body):
    req = urllib.request.Request(base + path, data=json.dumps(body).encode(), headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=300) as r:
        return json.load(r)


def med(v):
    return st.median(v) if v else None


def main():
    base, prefix, cases_file, out = sys.argv[1:5]
    cases = {c["id"]: c for c in json.load(open(cases_file))["cases"]}
    rows = []
    import os
    for net in [n for n in CDP if n in os.environ.get("BCT_NETS", ",".join(CDP)).split(",")]:
        try:
            data = json.load(open(f"results/{prefix}-{net}.json"))["rows"]
        except FileNotFoundError:
            continue
        for r in data:
            c = r["case"]
            b = r.get("browser") or {}
            ms = b.get("ms") or []
            if not ms:
                continue
            cold = c["browser_state"] == "cold"
            idx = [0] if cold else list(range(1, len(ms))) or [0]  # cold = the first run only; warm = the later runs (the first warms the file)
            req = {"source": c["dataset"], "kind": c["kind"], "bbox": c["bbox"], "crs": c["crs"], "client": {"warm": not cold}}
            for k in ("limit", "group_by", "filter"):
                if c.get(k) is not None:
                    req[k] = c[k]
            plan = post(base, "/api/query/plan", req)["physical"]
            sc = plan["scans"][0]
            acc = plan["assessment"]["browser_access"][0]
            ph = [b["phases"][i] for i in idx if b["phases"][i]]
            def pm(key):
                return med([p[key] for p in ph])
            rows.append({
                "id": c["id"], "net": net, "cold": cold, "dataset": c["dataset"], "kind": c["kind"], "limit": c.get("limit"), "fraction": c["fraction"],
                "strategy": acc["strategy"], "file_bytes": acc["file_bytes"],
                "est_requests": acc["requests_est"], "est_fetch_bytes": acc["fetch_bytes_est"],
                "kept_rg": len(sc["prune"]["kept"]), "total_rg": sc["prune"]["total_row_groups"], "kept_bytes": sc["prune"]["kept_bytes"],
                "covering_bytes": sc["covering_bytes_total"], "read_class": sc["read_class"], "total_rows": sc["total_rows"],
                "geom_row_bytes": sc.get("geometry_row_bytes"), "attr_row_bytes": sc.get("attribute_row_bytes"),
                "sel_geom_bytes": sc.get("selected_geometry_bytes"), "raw_selected_rows": sc.get("raw_selected_rows"),
                "est_rows": plan["result_rows_estimate"], "est_cardinality": plan["assessment"]["estimate"]["cardinality"],
                "rows": b.get("rows"), "meas_requests": med([b["file_requests"][i] for i in idx]), "meas_bytes": med([b["file_bytes"][i] for i in idx]),
                "total_ms": med([ms[i] for i in idx]), "plan_ms": med([b["plan_ms"][i] for i in idx]),
                "init_ms": pm("init_ms"), "open_ms": pm("open_ms"), "first_ms": pm("first_ms"), "drain_ms": pm("drain_ms"), "boundary_ms": pm("boundary_ms"),
                "planner_server_pred_ms": next((k["score_ms"] for k in plan["candidates"] if k["engine"] == "duckdb" and k["feasible"]), None),
                "planner_pred_ms": next((k["score_ms"] for k in plan["candidates"] if k["engine"] == "duckdb-wasm" and k["feasible"]), None),
                "server_ms": (r.get("server") or {}).get("median"),
            })
    json.dump(rows, open(out, "w"), indent=1)
    print(len(rows), "rows ->", out)


if __name__ == "__main__":
    main()
