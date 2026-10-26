"""Evaluation of the REAL planner's decisions on a measured set (metric-semijoin-policy-v1).

Every record of a benchmark run carries what the planner decided (`planner.chosen`, its candidates, its uncertainty) and both engines' measured times, so
nothing is replayed in Python: regret is `time(chosen) / time(best)`, a timeout is a CENSORED 60 s (a lower bound: the true time is longer), and a case is
decidable only when both engines produced an outcome that can be compared (identical results, or one of them timed out).

  python benchmarks/metric_safety_eval.py --set raw/msj_conf.jsonl --validation <validation json> [--force class=raw/plans-forced.jsonl ...] --out eval.json

`--force class=file` adds the class-promotion experiment: the file is the plan-only output of the same cases planned as if that PostGIS class were
validated (GF_COST_FORCE_VALIDATED=class). The class is judged on the cases of that class with the pre-committed gates (benchmarks/postgis_cost_gates.json).
"""

from __future__ import annotations

import argparse
import json
import statistics
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from postgis_cost_fit import pct  # noqa: E402
from postgis_cost_lib import load_raw  # noqa: E402

GATES = json.loads((Path(__file__).resolve().parent / "postgis_cost_gates.json").read_text(encoding="utf-8"))
T = 60_000.0
OLD_POLICY_VALIDATED = {"indexed_bbox", "aggregate", "semijoin_intersects"}  # the scope approved before this phase (postgis-cost-v2)


def t_of(rec: dict, e: str) -> float | None:
    x = rec[e]
    return x["model_ms"] if x["status"] == "ok" and x.get("model_ms") is not None else (T if x["status"] == "timeout" else None)


def decidable(rec: dict) -> bool:
    a, b = t_of(rec, "duckdb"), t_of(rec, "postgis")
    return a is not None and b is not None and (bool(rec.get("identical")) or rec["duckdb"]["status"] == "timeout" or rec["postgis"]["status"] == "timeout")


def cands(planner: dict) -> dict:
    return {c["engine"]: c for c in (planner or {}).get("candidates", [])}


def pg_class(planner: dict) -> str | None:
    cm = (cands(planner).get("postgis") or {}).get("cost_model")
    return cm["workload_class"] if cm else None


def risk_level(planner: dict, e: str) -> str | None:
    cm = (cands(planner).get(e) or {}).get("cost_model")
    return cm["risk"]["level"] if cm else None


def summary(rows: list[dict]) -> dict:
    if not rows:
        return {"n": 0}
    reg = [r["regret"] for r in rows]
    pen = [r["penalty_ms"] for r in rows]
    return {"n": len(rows), "p50_regret": round(statistics.median(reg), 3), "p90_regret": round(pct(reg, 0.9), 3), "p95_regret": round(pct(reg, 0.95), 3), "max_regret": round(max(reg), 2),
            "mean_penalty_ms": round(sum(pen) / len(pen), 1), "max_penalty_ms": round(max(pen), 1), "optimal_share": round(sum(1 for x in reg if x <= 1.05) / len(reg), 3)}


def judged(recs: list[dict], chooser) -> tuple[list[dict], list[dict]]:
    """(decision rows, unsafe selections). `chooser(rec) -> engine`."""
    rows, unsafe = [], []
    for r in recs:
        if not decidable(r):
            continue
        a, b = t_of(r, "duckdb"), t_of(r, "postgis")
        eng = chooser(r)
        if eng not in ("duckdb", "postgis"):
            continue
        sel, other = (a, b) if eng == "duckdb" else (b, a)
        best = min(a, b)
        rows.append({"id": r["id"], "cls": pg_class(r["planner"]) or "none", "engine": eng, "sel_ms": sel, "best_ms": best, "regret": sel / max(best, 1.0), "penalty_ms": sel - best,
                     "timeout": sel >= T, "alt_completed": other < T})
        if sel >= T and other < T:
            unsafe.append({"id": r["id"], "why": f"{eng} chosen, timed out at {T / 1000:.0f} s while the other engine completed in {other / 1000:.1f} s"})
        cand = cands(r["planner"]).get(eng) or {}
        if risk_level(r["planner"], eng) == "timeout_risk" and r["planner"].get("uncertainty") is None:
            unsafe.append({"id": r["id"], "why": f"{eng} chosen although the planner rated it timeout_risk and did not declare itself uncertain"})
        if cand and not cand.get("feasible", True):
            unsafe.append({"id": r["id"], "why": f"{eng} chosen but infeasible"})
    return rows, unsafe


def pred_ratio(recs: list[dict], floor: float) -> dict:
    v = []
    for r in recs:
        cm = (cands(r["planner"]).get("postgis") or {}).get("cost_model")
        a = t_of(r, "postgis")
        if cm and a is not None and a < T:
            p, t = max(cm["time_ms"], floor), max(a, floor)
            v.append(max(p / t, t / p))
    return {"n": len(v), "median": round(statistics.median(v), 2), "p90": round(pct(v, 0.9), 2), "p95": round(pct(v, 0.95), 2), "max": round(max(v), 1)} if v else {"n": 0}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--set", type=Path, required=True)
    ap.add_argument("--validation", type=Path, required=True)
    ap.add_argument("--force", nargs="*", default=[])
    ap.add_argument("--out", type=Path, required=True)
    a = ap.parse_args()
    recs = load_raw(a.set)
    val = json.loads(a.validation.read_text(encoding="utf-8"))
    validated = {c for c, v in val["classes"].items() if v["level"] == "validated"}
    dec = [r for r in recs if decidable(r)]
    out = {"cases": len(recs), "decidable": len(dec), "validated_scope": sorted(validated),
           "failures": {e: {s: sum(1 for r in recs if r[e]["status"] == s) for s in ("ok", "timeout", "error")} for e in ("duckdb", "postgis")}, "correctness_mismatches": sum(1 for r in recs if r.get("comparable") and not r.get("identical"))}

    real_rows, real_unsafe = judged(recs, lambda r: r["planner"].get("chosen"))
    out["planner"] = {"all": summary(real_rows), "in_validated_scope": summary([x for x in real_rows if x["cls"] in validated]), "outside_scope": summary([x for x in real_rows if x["cls"] not in validated]),
                      "unsafe": real_unsafe, "uncertain_plans": sum(1 for r in dec if r["planner"].get("uncertainty"))}
    out["timeouts_by_chosen_engine"] = {e: {"timeouts": sum(1 for x in real_rows if x["engine"] == e and x["timeout"]), "chosen": sum(1 for x in real_rows if x["engine"] == e),
                                            "alternate_engine_completed": sum(1 for x in real_rows if x["engine"] == e and x["timeout"] and x["alt_completed"])} for e in ("duckdb", "postgis")}
    old_rows, old_unsafe = judged(recs, lambda r: "postgis" if pg_class(r["planner"]) in OLD_POLICY_VALIDATED else "duckdb")
    out["baselines"] = {
        "previous_policy (class-scoped R5d, validated indexed_bbox/aggregate/semijoin_intersects, no R5e)": {**summary(old_rows), "unsafe": len(old_unsafe), "timeouts": sum(1 for x in old_rows if x["timeout"])},
        "always_duckdb": {**summary(judged(recs, lambda r: "duckdb")[0]), "timeouts": sum(1 for r in dec if t_of(r, "duckdb") >= T)},
        "always_postgis": {**summary(judged(recs, lambda r: "postgis")[0]), "timeouts": sum(1 for r in dec if t_of(r, "postgis") >= T)},
        "oracle_fastest": {"n": len(dec), "p90_regret": 1.0},
    }
    # alternate-engine-saved: the previous policy lost to a timeout, the new one did not
    new_by_id = {x["id"]: x for x in real_rows}
    old_by_id = {x["id"]: x for x in old_rows}
    out["alternate_engine_saved_cases"] = [i for i, o in old_by_id.items() if o["timeout"] and o["alt_completed"] and i in new_by_id and not new_by_id[i]["timeout"]]
    by_cls = {}
    for c in sorted({x["cls"] for x in real_rows}):
        sub = [r for r in dec if pg_class(r["planner"]) == c]
        by_cls[c] = {"planner": summary([x for x in real_rows if x["cls"] == c]), "prediction_ratio_10ms_floor": pred_ratio(sub, 10.0), "prediction_ratio_1ms_floor": pred_ratio(sub, 1.0)}
    out["by_class"] = by_cls

    g = GATES["class_auto_select_requires"]
    promo = {}
    for spec in a.force:
        cls, path = spec.split("=", 1)
        plans = {p["id"]: p["planner"] for p in load_raw(Path(path))}
        sub = [dict(r, planner=plans[r["id"]]) for r in recs if pg_class(r["planner"]) == cls and r["id"] in plans]
        rows, unsafe = judged(sub, lambda r: r["planner"].get("chosen"))
        s = summary(rows)
        pr = pred_ratio(sub, GATES["class_auto_select_requires"].get("prediction_ratio_floor_ms", 10))
        fails = []
        if s.get("n", 0) < g["min_heldout_cases"]:
            fails.append(f"only {s.get('n', 0)} cases (< {g['min_heldout_cases']})")
        else:
            if s["p90_regret"] > g["max_p90_regret"]:
                fails.append(f"P90 regret {s['p90_regret']} > {g['max_p90_regret']}")
            if pr.get("p90", 99) > g["max_p90_prediction_ratio"]:
                fails.append(f"P90 prediction ratio {pr.get('p90')} > {g['max_p90_prediction_ratio']}")
            if pr.get("median", 99) > g["max_median_prediction_ratio"]:
                fails.append(f"median prediction ratio {pr.get('median')} > {g['max_median_prediction_ratio']}")
        if unsafe:
            fails.append(f"{len(unsafe)} unsafe selection(s)")
        promo[cls] = {"if_validated": s, "prediction_ratio": pr, "unsafe": unsafe, "fails": fails, "passes_gates": not fails}
    out["promotion_experiments"] = promo
    a.out.write_text(json.dumps(out, indent=2) + "\n", encoding="utf-8", newline="\n")
    print(json.dumps({k: out[k] for k in ("cases", "decidable", "failures", "correctness_mismatches", "planner", "timeouts_by_chosen_engine", "baselines", "alternate_engine_saved_cases")}, indent=1)[:4500])
    for c, v in promo.items():
        print(f"{c}: {'PASSES' if v['passes_gates'] else 'fails'} {v['fails']} | regret {v['if_validated']} | pred {v['prediction_ratio']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
