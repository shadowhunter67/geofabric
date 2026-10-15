#!/usr/bin/env python3
"""Held-out planner evaluation: estimator error, time-model error, planner accuracy and regret, confidence calibration.

Inputs  results/heldout-<NET>.json (one per network profile; written by web/e2e/heldout.spec.ts)
        optional: a gateway WITHOUT calibration (--uncalibrated URL) to evaluate the v1-equivalent estimator on the same cases
Outputs results/heldout-summary.json and results/summary-planner-hardening.md

Definitions (fixed before looking at the data, see ADR-010):
  actual runtime of an engine = median end-to-end wall time through the (simulated) network, warm-up discarded
  fastest                      = the engine with the lowest measured time among those that completed
  selected_runtime             = measured time of the engine the planner chose by default
  correct                      = chosen == fastest;   regret = selected_runtime / fastest_runtime  (>= 1)
  error factor                 = max(est+1, actual+1) / min(est+1, actual+1)
"""
import argparse
import glob
import json
import math
import os
import statistics
import urllib.request


def q(v, p):
    v = sorted(v)
    return v[min(len(v) - 1, math.ceil((len(v) - 1) * p))]


def ef(est, act):
    e, a = est + 1.0, act + 1.0
    return max(e / a, a / e)


def summary(factors, over=None, cover=None):
    if not factors:
        return None
    s = {"n": len(factors), "median": round(statistics.median(factors), 2), "p90": round(q(factors, 0.9), 2), "p95": round(q(factors, 0.95), 2), "max": round(max(factors), 2)}
    if over is not None:
        s["over_rate"] = round(sum(1 for o in over if o > 0) / len(over), 2)
        s["under_rate"] = round(sum(1 for o in over if o < 0) / len(over), 2)
        s["large_under_rate_gt5x"] = round(sum(1 for f, o in zip(factors, over) if o < 0 and f > 5) / len(over), 3)
    if cover is not None:
        s["interval_coverage"] = round(sum(cover) / len(cover), 2)
    return s


def post(base, path, body):
    req = urllib.request.Request(base + path, data=json.dumps(body).encode(), headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=120) as r:
        return json.load(r)


def load(results_dir, prefix="heldout"):
    rows = []
    for f in sorted(glob.glob(os.path.join(results_dir, prefix + "-*.json"))):
        if f.endswith("-summary.json") or f.endswith("-cases.json") or "range-features" in f:
            continue
        d = json.load(open(f))
        if "rows" not in d:
            continue
        rows += [dict(r, network=d["network"]) for r in d["rows"]]
    return rows


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--results", default=os.path.join(os.path.dirname(__file__), "results"))
    ap.add_argument("--prefix", default="heldout")
    ap.add_argument("--cardinality", help="results/heldout-cardinality.json from eval_cardinality.py")
    ap.add_argument("--uncalibrated", help="gateway without telemetry, for the v1-equivalent estimator")
    a = ap.parse_args()
    rows = load(a.results, a.prefix)
    ok = [r for r in rows if "planner" in r and r.get("server", {}).get("median") is not None]
    out = {"cases_total": len(rows), "cases_evaluated": len(ok)}

    # ---------- estimator (result rows) ----------
    est_rows = []
    for r in ok:
        p, s = r["planner"], r["server"]
        e = p["estimate"]
        est_rows.append({"id": r["case"]["id"], "dataset": r["dataset"], "kind": r["case"]["kind"], "limit": r["case"]["limit"], "fraction": r["case"]["fraction"],
                         "actual": s["rows"], "est": e["rows"], "low": e["low"], "high": e["high"], "quality": e["quality"], "basis": e["correction"]["basis"]})
    v2 = [ef(x["est"], x["actual"]) for x in est_rows]
    out["estimator_v2"] = summary(v2, [x["est"] - x["actual"] for x in est_rows], [x["low"] <= x["actual"] <= x["high"] for x in est_rows])
    out["estimator_v2_by_kind"] = {k: summary([ef(x["est"], x["actual"]) for x in est_rows if x["kind"] == k]) for k in sorted({x["kind"] for x in est_rows})}
    out["estimator_v2_by_quality"] = {k: summary([ef(x["est"], x["actual"]) for x in est_rows if x["quality"] == k]) for k in sorted({x["quality"] for x in est_rows})}
    if a.uncalibrated:
        v1 = []
        for r in ok:
            c = r["case"]
            req = {"source": c["dataset"], "kind": c["kind"], "bbox": c["bbox"], "crs": c["crs"], "limit": c["limit"], "group_by": c["group_by"], "filter": c["filter"]}
            e = post(a.uncalibrated, "/api/query/plan", req)["physical"]["assessment"]["estimate"]
            v1.append((e["rows"], r["server"]["rows"], e["low"], e["high"]))
        out["estimator_uncalibrated"] = summary([ef(e, t) for e, t, *_ in v1], [e - t for e, t, *_ in v1], [lo <= t <= hi for e, t, lo, hi in v1])
    out["estimator_v2_cases"] = est_rows
    if a.cardinality:
        card = json.load(open(a.cardinality))
        cs = [card[x["id"]] for x in est_rows if x["id"] in card]
        out["cardinality"] = summary([ef(c["est"], c["actual"]) for c in cs], [c["est"] - c["actual"] for c in cs], [c["low"] <= c["actual"] <= c["high"] for c in cs])
        out["cardinality_above_interval_rate"] = round(sum(1 for c in cs if c["actual"] > c["high"]) / max(1, len(cs)), 3)
        out["cardinality_by_layout"] = {k: summary([ef(c["est"], c["actual"]) for c in cs if c["layout"] == k]) for k in sorted({c["layout"] for c in cs})}
        out["cardinality_cases"] = [dict(id=x["id"], **card[x["id"]]) for x in est_rows if x["id"] in card]

    # ---------- time model ----------
    te = {"duckdb": [], "duckdb-wasm": []}
    te_over = {"duckdb": [], "duckdb-wasm": []}
    for r in ok:
        for k in r["planner"]["candidates"]:
            m = r["server"]["median"] if k["engine"] == "duckdb" else (r.get("browser") or {}).get("median")
            if k["feasible"] and k["score_ms"] is not None and m is not None:
                te[k["engine"]].append(ef(k["score_ms"], m))
                te_over[k["engine"]].append(k["score_ms"] - m)
    out["time_model"] = {e: summary(v, te_over[e]) for e, v in te.items()}
    out["time_model_by_network"] = {}
    for net in sorted({r["network"] for r in ok}):
        d = {"duckdb": [], "duckdb-wasm": []}
        for r in [x for x in ok if x["network"] == net]:
            for k in r["planner"]["candidates"]:
                m = r["server"]["median"] if k["engine"] == "duckdb" else (r.get("browser") or {}).get("median")
                if k["feasible"] and k["score_ms"] is not None and m is not None:
                    d[k["engine"]].append(ef(k["score_ms"], m))
        out["time_model_by_network"][net] = {e: summary(v) for e, v in d.items()}

    # ---------- planner accuracy / regret ----------
    cases = []
    for r in ok:
        s = r["server"]["median"]
        b = (r.get("browser") or {})
        bm = b.get("median")
        measured = {"duckdb": s}
        if bm is not None and not b.get("error") and not b.get("crashed"):
            measured["duckdb-wasm"] = bm
        fastest = min(measured, key=measured.get)
        chosen = r["planner"]["engine"]
        if chosen not in measured:  # chosen the browser but it could not be measured: count as a failure of the decision
            sel = None
        else:
            sel = measured[chosen]
        cases.append({"id": r["case"]["id"], "network": r["network"], "dataset": r["dataset"], "kind": r["case"]["kind"], "fraction": r["case"]["fraction"],
                      "state": r["case"]["browser_state"], "chosen": chosen, "fastest": fastest, "server_ms": s, "browser_ms": bm,
                      "browser_status": ("measured" if bm is not None else (b.get("skipped") and "skipped (tab limit)") or (b.get("not_placed") and "not placed") or (b.get("error") and "error") or "n/a"),
                      "correct": chosen == fastest, "selected_ms": sel, "regret": (sel / measured[fastest]) if sel else None,
                      "confidence": r["planner"]["confidence"], "near_tie": len(measured) == 2 and abs(s - bm) / min(s, bm) < 0.10,
                      "pred_server": next((k["score_ms"] for k in r["planner"]["candidates"] if k["engine"] == "duckdb"), None),
                      "pred_browser": next((k["score_ms"] for k in r["planner"]["candidates"] if k["engine"] == "duckdb-wasm" and k["feasible"]), None)})
    n = len(cases)
    reg = [c["regret"] for c in cases if c["regret"] is not None]
    out["planner"] = {"cases": n, "accuracy": round(sum(c["correct"] for c in cases) / n, 3), "regret": summary(reg),
                      "regret_p90": round(q(reg, 0.9), 3) if reg else None, "unmeasurable_choice": sum(1 for c in cases if c["regret"] is None)}
    nt = [c for c in cases if not c["near_tie"]]
    out["planner"]["accuracy_excluding_near_ties"] = round(sum(c["correct"] for c in nt) / len(nt), 3) if nt else None
    out["planner"]["near_ties"] = n - len(nt)
    both = [c for c in cases if c["browser_ms"] is not None]
    out["planner"]["cases_with_both_engines_measured"] = len(both)
    out["planner"]["accuracy_on_both_measured"] = round(sum(c["correct"] for c in both) / len(both), 3) if both else None
    out["planner"]["browser_fastest"] = sum(1 for c in cases if c["fastest"] == "duckdb-wasm")
    out["planner"]["browser_chosen"] = sum(1 for c in cases if c["chosen"] == "duckdb-wasm")
    # baselines
    def baseline(engine):
        rr = []
        for c in cases:
            m = c["server_ms"] if engine == "duckdb" else c["browser_ms"]
            if m is not None:
                rr.append(m / min(x for x in (c["server_ms"], c["browser_ms"]) if x is not None))
        return {"cases": len(rr), "regret_p90": round(q(rr, 0.9), 3) if rr else None, "regret_median": round(statistics.median(rr), 3) if rr else None}
    out["baseline_always_server"] = {"accuracy": round(sum(1 for c in cases if c["fastest"] == "duckdb") / n, 3), **baseline("duckdb")}
    out["baseline_always_browser_where_measurable"] = baseline("duckdb-wasm")
    out["by_network"] = {net: {"cases": sum(1 for c in cases if c["network"] == net), "accuracy": round(sum(c["correct"] for c in cases if c["network"] == net) / max(1, sum(1 for c in cases if c["network"] == net)), 3),
                               "browser_fastest": sum(1 for c in cases if c["network"] == net and c["fastest"] == "duckdb-wasm")} for net in sorted({c["network"] for c in cases})}
    out["by_state"] = {st: {"cases": sum(1 for c in cases if c["state"] == st), "accuracy": round(sum(c["correct"] for c in cases if c["state"] == st) / max(1, sum(1 for c in cases if c["state"] == st)), 3)} for st in ("warm", "cold")}
    # confidence calibration
    bins = [(0, 0.5), (0.5, 0.65), (0.65, 0.8), (0.8, 1.01)]
    out["confidence_bins"] = [{"range": f"{lo:.2f}-{min(hi, 1):.2f}", "cases": sum(1 for c in cases if lo <= c["confidence"] < hi),
                               "accuracy": (round(sum(c["correct"] for c in cases if lo <= c["confidence"] < hi) / max(1, sum(1 for c in cases if lo <= c["confidence"] < hi)), 2)),
                               "mean_confidence": (round(statistics.mean([c["confidence"] for c in cases if lo <= c["confidence"] < hi]), 2) if any(lo <= c["confidence"] < hi for c in cases) else None)}
                              for lo, hi in bins]
    # browser stability / safety
    br = [r.get("browser") or {} for r in ok]
    out["browser"] = {"runs_attempted": sum(1 for b in br if b.get("ms")), "crashes": sum(1 for b in br if b.get("crashed")), "errors": sum(1 for b in br if b.get("error")),
                      "skipped_tab_limit": sum(1 for b in br if b.get("skipped")), "not_placed": sum(1 for b in br if b.get("not_placed")),
                      "digest_mismatch": sum(1 for b in br if b.get("digest_match") is False), "row_mismatch": sum(1 for b in br if b.get("rows_match") is False),
                      "digest_checked": sum(1 for b in br if b.get("digest_match") is not None)}
    # safety: browser chosen by default although it later crashed/failed
    out["unsafe_choices"] = [c["id"] for c, r in zip(cases, ok) if c["chosen"] == "duckdb-wasm" and ((r.get("browser") or {}).get("crashed") or (r.get("browser") or {}).get("error"))]
    out["cases"] = cases
    json.dump(out, open(os.path.join(a.results, a.prefix + "-summary.json"), "w"), indent=1)
    print(json.dumps({k: v for k, v in out.items() if k not in ("cases", "estimator_v2_cases")}, indent=1))


if __name__ == "__main__":
    main()
