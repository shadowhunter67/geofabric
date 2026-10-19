#!/usr/bin/env python3
"""Fit the browser cost model (planner hardening III) on the CALIBRATION table only and report how well each part and the sum fit.

The model is a sum of parts the engine itself measures (phases): plan round trip, engine start, opening the sources, the
requests/transfer, Parquet decode + compute, result materialisation and the WKB -> GeoJSON boundary. Each part has its own
coefficients and its own error. Quantities the planner must predict (requests, bytes) are fitted separately from planner-known
features, and the total is evaluated with PREDICTED quantities (the planner never sees measured ones).

  python fit_browser_model.py results/bcal-table.json > results/browser-model-fit.json

Held-out data is never read here. Leave-one-dataset-out error is reported to show the fit is not memorising a dataset.
"""
import json
import statistics as st
import sys

import numpy as np

from nnls_fit import fit

M = 1048576
WASM_BYTES = 36.7 * M  # duckdb-eh.wasm (35.9 MB) + its worker script: what a cold start downloads when the browser has no cached copy
NET = {"LOCAL": (0.0, 8000e6 / 8), "FAST_LAN": (5.0, 1000e6 / 8), "GOOD_BROADBAND": (20.0, 200e6 / 8), "MOBILE": (70.0, 30e6 / 8), "HIGH_LATENCY": (150.0, 20e6 / 8)}


def prep(rows):
    for r in rows:
        r["rtt"], r["down"] = NET[r["net"]]
        r["thr"] = 1.0 if r["rtt"] > 0 else 0.0
        r["aggregate"] = r["kind"] != "features"
        r["typed"] = r["read_class"] != "plain_wkb"
    return rows


def f(x, p):
    return max(x / p, p / x)


def summarize(pairs):
    fa = [f(max(a, 1.0), max(b, 1.0)) for a, b in pairs]
    return {"n": len(fa), "median": round(st.median(fa), 3), "p90": round(float(np.percentile(fa, 90)), 3), "max": round(max(fa), 2)}


def fit_quantities(rows):
    """Requests and bytes of range mode from planner-known features (row groups kept, their bytes, covering bytes, limit)."""
    out = {}
    rng = [r for r in rows if r["strategy"] == "range"]
    # aggregates: requests = a + b * kept_rg ; bytes = share * kept_bytes (plain WKB), typed files read the covering/group columns
    ag = [r for r in rng if r["aggregate"] and not r["typed"]]
    c = fit([[1, r["kept_rg"]] for r in ag], [max(r["meas_requests"], 1) for r in ag])
    out["agg_requests_const"], out["agg_requests_per_rg"] = float(c[0]), float(c[1])
    ratio = [r["meas_bytes"] / max(r["kept_bytes"], 1) for r in ag if r["kept_bytes"] > M]
    out["agg_plain_bytes_share"] = float(st.median(ratio))
    ag_t = [r for r in rng if r["aggregate"] and r["typed"]]
    c = fit([[1, r["kept_rg"]] for r in ag_t], [max(r["meas_requests"], 1) for r in ag_t])
    out["agg_typed_requests_const"], out["agg_typed_requests_per_rg"] = float(c[0]), float(c[1])
    ratio = [r["meas_bytes"] / max(r["covering_bytes"] * r["kept_rg"] / max(r["total_rg"], 1), 1) for r in ag_t]
    out["agg_typed_bytes_per_covering_share"] = float(st.median(ratio))
    # features: LIMIT-bound (the estimate says the limit is reached) vs not
    ft = [r for r in rng if not r["aggregate"]]
    bound = [r for r in ft if r["limit"] and (r["est_cardinality"] or 0) >= r["limit"] and r["rows"] and r["rows"] >= r["limit"] * 0.99]
    free = [r for r in ft if r not in bound]
    out["feat_bound_requests"] = float(st.median([r["meas_requests"] for r in bound])) if bound else 14.0
    out["feat_bound_bytes_mb"] = float(st.median([r["meas_bytes"] / M for r in bound])) if bound else 6.0
    c = fit([[1, r["kept_rg"]] for r in free], [max(r["meas_requests"], 1) for r in free])
    out["feat_free_requests_const"], out["feat_free_requests_per_rg"] = float(c[0]), float(c[1])
    ratio = [r["meas_bytes"] / max(r["kept_bytes"], 1) for r in free if r["kept_bytes"] > M]
    out["feat_free_bytes_share"] = float(st.median(ratio)) if ratio else 1.5
    out["n"] = {"agg_plain": len(ag), "agg_typed": len(ag_t), "feat_bound": len(bound), "feat_free": len(free)}
    return out


def predict_quantities(r, q):
    """-> (requests, bytes) the planner would expect in range mode."""
    if r["aggregate"]:
        if r["typed"]:
            share = r["covering_bytes"] * r["kept_rg"] / max(r["total_rg"], 1)
            return q["agg_typed_requests_const"] + q["agg_typed_requests_per_rg"] * r["kept_rg"], q["agg_typed_bytes_per_covering_share"] * share
        return q["agg_requests_const"] + q["agg_requests_per_rg"] * r["kept_rg"], q["agg_plain_bytes_share"] * r["kept_bytes"]
    is_bound = r["limit"] and (r["est_cardinality"] or 0) >= r["limit"]
    if is_bound:
        return q["feat_bound_requests"], min(q["feat_bound_bytes_mb"] * M, max(r["kept_bytes"], 0.1 * M) * 1.2)
    return q["feat_free_requests_const"] + q["feat_free_requests_per_rg"] * r["kept_rg"], q["feat_free_bytes_share"] * r["kept_bytes"]


def fit_time(rows):
    p = {}
    # plan round trip
    c = fit([[1, r["rtt"], r["thr"]] for r in rows], [r["plan_ms"] for r in rows])
    p["plan_ms"], p["rtt_factor"], p["turn_overhead_ms"] = map(float, c)
    # cold start: engine instantiation (+ the engine assets when the browser has no cached copy) and the first-query warm-up
    cold = [r for r in rows if r["cold"]]
    c = fit([[1, WASM_BYTES / r["down"] * 1000, r["rtt"], r["thr"]] for r in cold], [r["init_ms"] for r in cold])
    p["init_cpu_ms"], p["asset_transfer_factor"], p["asset_rtt_factor"], p["asset_overhead_ms"] = map(float, c)
    warm_first = {s: [r["first_ms"] for r in rows if not r["cold"] and r["strategy"] == s and (r["rows"] or 0) < 2000 and r["net"] == "LOCAL"] for s in ("full", "range")}
    cold_first = {s: [r["first_ms"] for r in cold if r["strategy"] == s and (r["rows"] or 0) < 2000 and r["net"] == "LOCAL"] for s in ("full", "range")}
    p["cold_first_query_ms"] = float(st.median([st.median(cold_first[s]) - st.median(warm_first[s]) for s in ("full", "range") if cold_first[s] and warm_first[s]]))
    # whole-file mode: open = download + registration; first = decode + compute
    full = [r for r in rows if r["strategy"] == "full"]
    c = fit([[1, r["rtt"], r["file_bytes"] / r["down"] * 1000, r["thr"]] for r in full], [max(r["open_ms"], 1) for r in full])
    p["full_open_ms"], p["full_open_rtt_factor"], p["full_transfer_factor"], p["full_open_overhead_ms"] = map(float, c)
    wf = [r for r in full if not r["cold"]]
    c = fit([[1, r["kept_bytes"] / M, (r["rows"] or 0) * (r["geom_row_bytes"] or 0) / M] for r in wf], [max(r["first_ms"], 1) for r in wf])
    p["full_first_ms"], p["full_first_ms_per_kept_mb"], p["full_first_ms_per_result_geom_mb"] = map(float, c)
    # range mode: per sequential request: cpu + (rtt + overhead when throttled); bytes at link speed
    rg = [r for r in rows if r["strategy"] == "range" and not r["cold"]]
    c = fit([[1, r["meas_requests"], r["meas_requests"] * r["rtt"], r["meas_requests"] * r["thr"], r["meas_bytes"] / r["down"] * 1000] for r in rg], [max(r["first_ms"], 1) for r in rg])
    p["range_first_ms"], p["range_request_cpu_ms"], p["range_request_rtt_factor"], p["range_request_overhead_ms"], p["range_transfer_factor"] = map(float, c)
    # result: materialise rows + convert to GeoJSON (geometry bytes dominate for heavy polygons)
    res = [r for r in rows if (r["rows"] or 0) >= 5]
    c = fit([[1, (r["rows"]) / 1000, r["rows"] * (r["geom_row_bytes"] or 0) / M, r["rows"] * (r["attr_row_bytes"] or 0) / M] for r in res], [max(r["drain_ms"] + r["boundary_ms"], 1) for r in res])
    p["result_ms"], p["result_ms_per_krow"], p["result_ms_per_geom_mb"], p["result_ms_per_attr_mb"] = map(float, c)
    return p


def predict_total(r, p, q, measured_quantities=False):
    """The planner's browser time for one case from planner-known inputs only (or measured quantities for diagnosis)."""
    turn = p["rtt_factor"] * r["rtt"] + p["turn_overhead_ms"] * r["thr"]
    t = p["plan_ms"] + turn
    if r["cold"]:
        t += p["init_cpu_ms"] + p["asset_transfer_factor"] * WASM_BYTES / r["down"] * 1000 + p["asset_rtt_factor"] * r["rtt"] + p["asset_overhead_ms"] * r["thr"] + p["cold_first_query_ms"]
    rows = r["rows"] or 0
    res = p["result_ms"] + p["result_ms_per_krow"] * rows / 1000 + p["result_ms_per_geom_mb"] * rows * (r["geom_row_bytes"] or 0) / M + p["result_ms_per_attr_mb"] * rows * (r["attr_row_bytes"] or 0) / M if rows >= 1 else 0.0
    if r["strategy"] == "full":
        t += p["full_open_ms"] + p["full_open_rtt_factor"] * r["rtt"] + p["full_transfer_factor"] * r["file_bytes"] / r["down"] * 1000 + p["full_open_overhead_ms"] * r["thr"]
        t += p["full_first_ms"] + p["full_first_ms_per_kept_mb"] * r["kept_bytes"] / M + p["full_first_ms_per_result_geom_mb"] * rows * (r["geom_row_bytes"] or 0) / M
    else:
        req, by = (r["meas_requests"], r["meas_bytes"]) if measured_quantities else predict_quantities(r, q)
        t += p["range_first_ms"] + req * (p["range_request_cpu_ms"] + p["range_request_rtt_factor"] * r["rtt"] + p["range_request_overhead_ms"] * r["thr"]) + p["range_transfer_factor"] * by / r["down"] * 1000
    return t + res


def evaluate(rows, p, q, label):
    pairs = [(r["total_ms"], predict_total(r, p, q)) for r in rows]
    out = {"all": summarize(pairs)}
    for key in ("net", "strategy", "cold", "kind"):
        groups = {}
        for r, pr in zip(rows, pairs):
            groups.setdefault(str(r[key]), []).append(pr)
        out["by_" + key] = {k: summarize(v) for k, v in groups.items()}
    return {label: out}


def main():
    rows = prep(json.load(open(sys.argv[1])))
    q = fit_quantities(rows)
    p = fit_time(rows)
    res = {"quantities": q, "time": p}
    res.update(evaluate(rows, p, q, "in_sample"))
    # diagnosis: how good is the time model if the quantities were known exactly?
    res["in_sample_with_measured_quantities"] = {"all": summarize([(r["total_ms"], predict_total(r, p, q, True)) for r in rows])}
    # leave-one-dataset-out (the dataset family: file name without the size/order/draw suffix)
    fam = lambda r: r["dataset"].split("_")[0] if not r["dataset"].startswith("overture") else r["dataset"]
    held = []
    for g in sorted({fam(r) for r in rows}):
        tr = [r for r in rows if fam(r) != g]
        te = [r for r in rows if fam(r) == g]
        if len(te) < 3 or len(tr) < 40:
            continue
        try:
            qq, pp = fit_quantities(tr), fit_time(tr)
        except Exception:
            continue
        held += [(r["total_ms"], predict_total(r, pp, qq)) for r in te]
    res["leave_one_dataset_family_out"] = summarize(held)
    print(json.dumps(res, indent=1))


if __name__ == "__main__":
    main()
