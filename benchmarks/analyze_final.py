#!/usr/bin/env python3
"""Final planner validation analysis.

  python benchmarks/analyze_final.py --prefix final [--out results/final-summary.json]

Inputs: results/<prefix>-<NET>.json (written by web/e2e/heldout.spec.ts). Everything is read from the run itself (the planner's
decision as the gateway made it, with its frozen model and policy; the measured times). Nothing is refitted here.

Three things are kept apart for every case (brief, section 7):
  model_fastest    the engine with the lowest cost-model score among the FEASIBLE candidates (what the cost model alone would pick)
  policy_selected  the engine the planner actually chose (cost model + confidence policy R5b/R5c)
  actual_fastest   the engine with the lowest MEASURED time (browser measured only when it could run safely: otherwise the server
                   is the safe winner by definition, reported as `browser_unmeasurable_safety`)
regret = time(policy_selected) / time(actual_fastest); the absolute penalty in ms is reported next to it.

Timing contract (docs/claude/pre-postgis-gate.md): browser time = plan request + response + engine start (cold only) + open + first
batch + drain + conversion, measured with one clock inside the page; server time = the same POST through the page, wall clock.
"""
import argparse
import glob
import json
import os
import statistics as st

import numpy as np

LARGE_BYTES = 128 << 20
NETS = ["LOCAL", "FAST_LAN", "GOOD_BROADBAND", "MOBILE", "HIGH_LATENCY"]


def f(a, b):
    a, b = max(a, 1.0), max(b, 1.0)
    return max(a / b, b / a)


def summ(v):
    if not v:
        return None
    v = sorted(v)
    pc = lambda p: float(np.percentile(v, p))
    return {"n": len(v), "median": round(st.median(v), 3), "p90": round(pc(90), 3), "p95": round(pc(95), 3), "max": round(max(v), 2)}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--prefix", required=True)
    ap.add_argument("--results", default=os.path.join(os.path.dirname(__file__), "results"))
    ap.add_argument("--out")
    a = ap.parse_args()
    rows = []
    for net in NETS:
        p = os.path.join(a.results, f"{a.prefix}-{net}.json")
        if os.path.exists(p):
            for r in json.load(open(p, encoding="utf-8"))["rows"]:
                r["net"] = net
                rows.append(r)
    cases = []
    for r in rows:
        pl, sv, br, c = r.get("planner"), r.get("server"), r.get("browser") or {}, r["case"]
        if not pl or not sv or sv.get("median") is None:
            continue
        cand = {k["engine"]: k for k in pl["candidates"]}
        feas = {e: k["score_ms"] for e, k in cand.items() if k["feasible"] and k["score_ms"] is not None}
        model_fastest = min(feas, key=feas.get) if feas else None
        acc = pl["browser_access"][0] if pl.get("browser_access") else {}
        file_bytes = acc.get("file_bytes", 0)
        large = file_bytes > LARGE_BYTES
        r5c = any(m.startswith("R5c") for m in (pl.get("policy") or []))
        b_med = br.get("median")
        measured_browser = b_med is not None and not br.get("skipped") and not br.get("not_placed") and not br.get("error")
        s_med = sv["median"]
        if measured_browser:
            actual = "duckdb-wasm" if b_med < s_med else "duckdb"
        else:
            actual = "duckdb"  # the browser could not run safely (or at all): the server is the only safe, measurable winner
        sel = pl["engine"]
        t_sel = (b_med if sel == "duckdb-wasm" else s_med) if (sel == "duckdb" or measured_browser) else None
        t_best = min(s_med, b_med) if measured_browser else s_med
        regret = (t_sel / t_best) if t_sel is not None and t_best else None
        cases.append({
            "id": c["id"], "net": r["net"], "dataset": c["dataset"], "kind": c["kind"], "state": c["browser_state"], "file_mb": round(file_bytes / 1048576, 1),
            "large": large, "size_class": c.get("size_class"), "strategy": acc.get("strategy"),
            "model_scores": feas, "model_fastest": model_fastest, "policy_selected": sel, "actual_fastest": actual, "r5c_fired": r5c,
            "policy_overrode_model": bool(model_fastest and model_fastest != sel), "policy_notes": [m[:160] for m in (pl.get("policy") or [])],
            "server_ms": s_med, "browser_ms": b_med if measured_browser else None,
            "browser_status": "measured" if measured_browser else (br.get("skipped") and "unmeasurable_safety") or (br.get("not_placed") and "not_placed") or (br.get("error") and "error") or "n/a",
            "browser_crashed": bool(br.get("crashed")),
            "regret": regret, "abs_penalty_ms": (t_sel - t_best) if t_sel is not None else None,
            "correct": sel == actual, "unsafe": bool(sel == "duckdb-wasm" and (large or br.get("crashed") or (br.get("skipped") is not None))),
            "browser_err": f(b_med, feas["duckdb-wasm"]) if measured_browser and "duckdb-wasm" in feas else None,
            "server_err": f(s_med, feas["duckdb"]) if "duckdb" in feas else None,
            "ref": r.get("reference"),
        })
    out = {"cases": len(cases), "prefix": a.prefix}

    def slice_stats(sel_fn, key):
        return summ([c[key] for c in cases if sel_fn(c) and c[key] is not None])

    out["browser_model_error"] = {"all": slice_stats(lambda c: True, "browser_err"),
                                  "by_net": {n: slice_stats(lambda c, n=n: c["net"] == n, "browser_err") for n in NETS},
                                  "by_size": {"<=128MiB": slice_stats(lambda c: not c["large"], "browser_err"), ">128MiB": slice_stats(lambda c: c["large"], "browser_err")},
                                  "by_strategy": {s: slice_stats(lambda c, s=s: c["strategy"] == s, "browser_err") for s in ("full", "range")},
                                  "by_state": {s: slice_stats(lambda c, s=s: c["state"] == s, "browser_err") for s in ("warm", "cold")}}
    out["server_model_error"] = {"all": slice_stats(lambda c: True, "server_err"), "by_size": {"<=128MiB": slice_stats(lambda c: not c["large"], "server_err"), ">128MiB": slice_stats(lambda c: c["large"], "server_err")},
                                 "pack_only": slice_stats(lambda c: c["size_class"] == "pack", "server_err")}
    reg = [c["regret"] for c in cases if c["regret"] is not None]
    pen = [c["abs_penalty_ms"] for c in cases if c["abs_penalty_ms"] is not None]
    out["planner"] = {
        "cases": len(cases), "accuracy": round(sum(c["correct"] for c in cases) / len(cases), 4),
        "always_server_accuracy": round(sum(c["actual_fastest"] == "duckdb" for c in cases) / len(cases), 4),
        "regret": {"p50": round(float(np.percentile(reg, 50)), 3), "p90": round(float(np.percentile(reg, 90)), 3), "p95": round(float(np.percentile(reg, 95)), 3), "max": round(max(reg), 3)},
        "abs_penalty_ms": {"p90": round(float(np.percentile(pen, 90)), 1), "p95": round(float(np.percentile(pen, 95)), 1), "max": round(max(pen), 1), "sum_wrong": round(sum(p for p in pen if p > 0), 1)},
        "unsafe_choices": [c["id"] for c in cases if c["unsafe"]], "browser_crashes": sum(c["browser_crashed"] for c in cases),
        "fallbacks": sum(1 for r in rows if (r.get("server") or {}).get("fallback")),
        "wrong_choices": [{k: c[k] for k in ("id", "net", "dataset", "kind", "state", "file_mb", "policy_selected", "actual_fastest", "server_ms", "browser_ms", "regret", "abs_penalty_ms")} for c in cases if not c["correct"]],
        "by_net": {n: round(sum(c["correct"] for c in cases if c["net"] == n) / max(1, sum(1 for c in cases if c["net"] == n)), 3) for n in NETS},
    }
    big = [c for c in cases if c["large"]]
    out["large_files"] = {
        "cases": len(big), "r5c_fired": sum(c["r5c_fired"] for c in big),
        "browser_selected": [c["id"] for c in big if c["policy_selected"] == "duckdb-wasm"],
        "model_would_pick_browser": sum(1 for c in big if c["model_fastest"] == "duckdb-wasm"),
        "policy_overrode_model": sum(1 for c in big if c["policy_overrode_model"]),
        "browser_measured": sum(1 for c in big if c["browser_status"] == "measured"), "browser_unmeasurable_safety": sum(1 for c in big if c["browser_status"] == "unmeasurable_safety"),
        "actual_browser_faster": [c["id"] for c in big if c["actual_fastest"] == "duckdb-wasm"],
        "regret": summ([c["regret"] for c in big if c["regret"] is not None]),
        "table": [{k: c[k] for k in ("id", "net", "dataset", "file_mb", "state", "model_scores", "model_fastest", "policy_selected", "actual_fastest", "r5c_fired", "server_ms", "browser_ms", "browser_status", "regret")} for c in big],
    }
    # drift control: reference probe per dataset batch (diagnostic only; the gates use the raw numbers)
    refs = [(c["net"], c["ref"]) for c in cases if c["ref"] and c["ref"].get("browser_ms") and c["ref"].get("server_ms")]
    if refs:
        by = {}
        for n, rf in refs:
            by.setdefault(n, []).append(rf)
        out["reference_probe"] = {n: {"batches": len(v), "server_ms_median": round(st.median(x["server_ms"] for x in v), 1), "browser_ms_median": round(st.median(x["browser_ms"] for x in v), 1),
                                      "server_ms_range": [round(min(x["server_ms"] for x in v), 1), round(max(x["server_ms"] for x in v), 1)], "browser_ms_range": [round(min(x["browser_ms"] for x in v), 1), round(max(x["browser_ms"] for x in v), 1)]} for n, v in by.items()}
        base = st.median(rf["browser_ms"] for _, rf in refs)
        norm = [f(c["browser_ms"], c["model_scores"]["duckdb-wasm"] * (c["ref"]["browser_ms"] / base)) for c in cases if c["browser_ms"] and "duckdb-wasm" in c["model_scores"] and c["ref"] and c["ref"].get("browser_ms")]
        out["browser_model_error_drift_normalised"] = summ(norm)
    out["browser_status_counts"] = {}
    for c in cases:
        out["browser_status_counts"][c["browser_status"]] = out["browser_status_counts"].get(c["browser_status"], 0) + 1
    out["digest_mismatch"] = [r["case"]["id"] for r in rows if (r.get("browser") or {}).get("digest_match") is False]
    out["row_mismatch"] = [r["case"]["id"] for r in rows if (r.get("browser") or {}).get("rows_match") is False]
    out["all_cases"] = cases
    path = a.out or os.path.join(a.results, f"{a.prefix}-summary.json")
    json.dump(out, open(path, "w", encoding="utf-8"), indent=1)
    show = {k: v for k, v in out.items() if k not in ("all_cases",)}
    show["large_files"] = {k: v for k, v in out["large_files"].items() if k != "table"}
    print(json.dumps(show, indent=1)[:9000])


if __name__ == "__main__":
    main()
