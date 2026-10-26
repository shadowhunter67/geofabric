"""Evaluate the PostGIS cost model on a measured set: planner regret, absolute penalty, baselines, prediction error, per-class eligibility.

The harness (crates/geo-runtime/tests/postgis_cost.rs) logs, for every case, the real planner's candidates (feasibility, score, the engine cost model's
class / time / risk) and both engines' measured times. This script re-applies the planner's selection policy over those logged candidates - the same
rules as geo-planner (feasible candidates; cheapest score; R5d: a candidate whose engine/class is not measured loses to the cheapest measured one) with
the validation map given on the command line - and compares the choice with the truth. The gates are benchmarks/postgis_cost_gates.json, fixed before
the held-out run.

    python benchmarks/postgis_cost_eval.py decide --set raw/heldout.jsonl --out models/postgis-validation.json     # which classes pass the gates
    python benchmarks/postgis_cost_eval.py report --set raw/confirm.jsonl --validation <final validation> --out confirm-eval.json
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
TIMEOUT_MS = 60_000.0


def actual(rec: dict, eng: str) -> float | None:
    """Measured time; a timed-out run is CENSORED: it took at least TIMEOUT_MS (so the regret of choosing it is a lower bound - conservative)."""
    e = rec[eng]
    if e["status"] == "ok" and e.get("model_ms") is not None:
        return e["model_ms"]
    if e["status"] == "timeout":
        return TIMEOUT_MS
    return None


def censored(rec: dict, eng: str) -> bool:
    return rec[eng]["status"] == "timeout"


def decidable(rec: dict) -> bool:
    """A case whose truth we know: both engines ran and agree, or one finished and the other timed out (it cannot be the better choice)."""
    a, b = actual(rec, "duckdb"), actual(rec, "postgis")
    if a is None or b is None:
        return False
    return bool(rec.get("identical")) or censored(rec, "duckdb") or censored(rec, "postgis")


def cands(rec: dict) -> dict[str, dict]:
    return {c["engine"]: c for c in (rec.get("planner") or {}).get("candidates", [])}


def pg_class(rec: dict) -> str | None:
    cm = (cands(rec).get("postgis") or {}).get("cost_model")
    return cm["workload_class"] if cm else None


def decide(rec: dict, validated: set[str], risk_applies: bool = True) -> tuple[str | None, str]:
    """Replay of the planner's choice for a logged case. `validated` = classes whose PostGIS model is Validated. Returns (engine, reason)."""
    cs = cands(rec)
    feas = {e: c for e, c in cs.items() if c["feasible"] and c["score_ms"] is not None}
    # the log was taken with an EMPTY validation file, i.e. the planner already applied R5d; re-derive the pre-R5d cheapest from scores
    if not feas:
        return None, "no feasible engine"
    cheapest = min(feas, key=lambda e: feas[e]["score_ms"])
    measured = [e for e in feas if e != "postgis" or (pg_class(rec) in validated)]
    if cheapest not in measured and measured:
        return min(measured, key=lambda e: feas[e]["score_ms"]), "R5d"
    return cheapest, "cost"


def metrics_for(recs: list[dict], validated: set[str]) -> dict:
    rows = []
    unsafe = []
    mismatches = 0
    for r in recs:
        if r.get("comparable") and not r.get("identical"):
            mismatches += 1
        a = {e: actual(r, e) for e in ("duckdb", "postgis")}
        ok = {e: t for e, t in a.items() if t is not None}
        if not ok or not decidable(r):
            continue  # nothing to compare: not a planning decision we can score
        best = min(ok.values())
        eng, why = decide(r, validated)
        if eng is None:
            continue
        sel = a.get(eng)
        if sel is None or censored(r, eng):
            # the selected engine produced nothing (or hit the timeout) while the other completed: unsafe by definition
            unsafe.append({"id": r["id"], "reason": f"{eng} selected but it {r[eng]['status']} while the other completed"})
            if sel is None:
                continue
        cs = cands(r)
        cm = (cs.get("postgis") or {}).get("cost_model") or {}
        if eng == "postgis" and cm.get("index_state") not in (None, "available"):
            unsafe.append({"id": r["id"], "reason": f"postgis selected with index state {cm.get('index_state')}"})
        if eng == "postgis" and (cm.get("risk") or {}).get("blocked") and not cs["postgis"]["feasible"]:
            unsafe.append({"id": r["id"], "reason": "postgis selected although the risk policy blocks it"})
        rows.append({"id": r["id"], "cls": pg_class(r) or "none", "template": r["template"], "engine": eng, "why": why, "sel_ms": sel, "best_ms": best, "regret": sel / max(best, 1.0), "penalty_ms": sel - best, "best_engine": min(ok, key=ok.get), "pg_pred": cm.get("time_ms"), "pg_act": a["postgis"], "dd_pred": (cs.get("duckdb") or {}).get("score_ms"), "dd_act": a["duckdb"]})
    return {"rows": rows, "unsafe": unsafe, "mismatches": mismatches}


def summarize(rows: list[dict]) -> dict:
    if not rows:
        return {"n": 0}
    reg = [r["regret"] for r in rows]
    pen = [r["penalty_ms"] for r in rows]
    return {"n": len(rows), "p50_regret": round(statistics.median(reg), 3), "p90_regret": round(pct(reg, 0.9), 3), "p95_regret": round(pct(reg, 0.95), 3), "max_regret": round(max(reg), 2),
            "max_penalty_ms": round(max(pen), 1), "mean_penalty_ms": round(sum(pen) / len(pen), 1), "optimal": sum(1 for r in rows if r["regret"] <= 1.05), "pg_chosen": sum(1 for r in rows if r["engine"] == "postgis")}


def baselines(recs: list[dict]) -> dict:
    out = {}
    for name, pick in (("always_duckdb", lambda r: "duckdb"), ("always_postgis", lambda r: "postgis"), ("existing_planner_R5d_everywhere", None)):
        rows = []
        failed = 0
        for r in recs:
            a = {e: actual(r, e) for e in ("duckdb", "postgis")}
            ok = {e: t for e, t in a.items() if t is not None}
            if not ok or not decidable(r):
                continue
            eng = pick(r) if pick else (decide(r, set())[0])
            if eng is None or a.get(eng) is None:
                # the engine fails or cannot run the case: the baseline is charged the cap, not skipped
                failed += 1
                rows.append({"regret": TIMEOUT_MS / max(min(ok.values()), 1.0), "penalty_ms": TIMEOUT_MS - min(ok.values()), "engine": eng})
                continue
            rows.append({"regret": a[eng] / max(min(ok.values()), 1.0), "penalty_ms": a[eng] - min(ok.values()), "engine": eng})
        s = summarize(rows)
        s["failed_or_capped"] = failed
        out[name] = s
    return out


def pred_ratio(recs: list[dict], cls: str | None = None, floor: float | None = None) -> dict:
    floor = GATES["class_auto_select_requires"].get("prediction_ratio_floor_ms", 1.0) if floor is None else floor
    v = []
    for r in recs:
        cm = (cands(r).get("postgis") or {}).get("cost_model")
        a = actual(r, "postgis")
        if not cm or a is None or (cls and cm["workload_class"] != cls):
            continue
        p, t = max(cm["time_ms"], floor), max(a, floor)
        v.append(max(p / t, t / p))
    if not v:
        return {"n": 0}
    return {"n": len(v), "median": round(statistics.median(v), 2), "p90": round(pct(v, 0.9), 2), "p95": round(pct(v, 0.95), 2), "max": round(max(v), 1)}


def classes_of(recs: list[dict]) -> list[str]:
    return sorted({pg_class(r) for r in recs if pg_class(r)})


def class_decision(recs: list[dict], cls: str) -> dict:
    """Would PostGIS be auto-selectable for `cls`? Evaluated by replaying the planner with ONLY this class validated."""
    g = GATES["class_auto_select_requires"]
    sub = [r for r in recs if pg_class(r) == cls and decidable(r)]
    m = metrics_for(sub, {cls})
    s = summarize(m["rows"])
    pr = pred_ratio(sub, cls)
    pr1 = pred_ratio(sub, cls, floor=1.0)
    index = {((cands(r).get("postgis") or {}).get("cost_model") or {}).get("index_state") for r in sub}
    fails = []
    if s.get("n", 0) < g["min_heldout_cases"]:
        fails.append(f"only {s.get('n', 0)} held-out cases (< {g['min_heldout_cases']})")
    else:
        if s["p90_regret"] > g["max_p90_regret"]:
            fails.append(f"P90 regret {s['p90_regret']} > {g['max_p90_regret']}")
        if pr.get("p90", 99) > g["max_p90_prediction_ratio"]:
            fails.append(f"P90 prediction ratio {pr.get('p90')} > {g['max_p90_prediction_ratio']}")
        if pr.get("median", 99) > g["max_median_prediction_ratio"]:
            fails.append(f"median prediction ratio {pr.get('median')} > {g['max_median_prediction_ratio']}")
    if m["unsafe"]:
        fails.append(f"{len(m['unsafe'])} unsafe selection(s)")
    if m["mismatches"]:
        fails.append(f"{m['mismatches']} correctness mismatch(es)")
    if index != {"available"}:
        fails.append(f"index state {sorted(i for i in index if i)} is not 'available'")
    return {"n": s.get("n", 0), "summary": s, "prediction_ratio": pr, "prediction_ratio_1ms": pr1, "unsafe": m["unsafe"], "fails": fails, "validated": not fails}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["decide", "report"])
    ap.add_argument("--set", type=Path, required=True)
    ap.add_argument("--validation", type=Path, help="report: the validation file whose classes count as validated")
    ap.add_argument("--plans", type=Path, help="plan-only JSONL (GF_COST_PLAN_ONLY=1): the planner view of the SAME cases under the current models; replaces the logged one")
    ap.add_argument("--model-version", default="postgis-cost-v1")
    ap.add_argument("--seed", type=int)
    ap.add_argument("--out", type=Path, required=True)
    a = ap.parse_args()
    recs = load_raw(a.set)
    if a.plans:
        plans = {p["id"]: p["planner"] for p in load_raw(a.plans)}
        for r in recs:
            r["planner"] = plans.get(r["id"], r.get("planner"))
    if a.cmd == "decide":
        out = {"model_version": a.model_version, "heldout_seed": a.seed or GATES["heldout_seed"], "classes": {}}
        for cls in classes_of(recs):
            d = class_decision(recs, cls)
            out["classes"][cls] = {
                "level": "validated" if d["validated"] else ("limited" if d["n"] else "unvalidated"),
                "heldout_cases": d["n"], "p90_regret": d["summary"].get("p90_regret"), "p90_prediction_ratio": d["prediction_ratio"].get("p90"), "p90_prediction_ratio_1ms_floor": d["prediction_ratio_1ms"].get("p90"),
                "median_prediction_ratio": d["prediction_ratio"].get("median"), "unsafe_selections": len(d["unsafe"]),
                "reason": "passes the gates (n, P90 regret, prediction error, no unsafe selection, index available)" if d["validated"] else "; ".join(d["fails"]),
            }
        a.out.write_text(json.dumps(out, indent=2) + "\n", encoding="utf-8", newline="\n")
        for c, v in out["classes"].items():
            print(f"{c:24s} {v['level']:11s} n={v['heldout_cases']:3d} p90_regret={v['p90_regret']} pred_p90={v['p90_prediction_ratio']} med={v['median_prediction_ratio']}  {v['reason']}")
        return 0
    val = json.loads(a.validation.read_text(encoding="utf-8"))
    validated = {c for c, v in val["classes"].items() if v["level"] == "validated"}
    m = metrics_for(recs, validated)
    overall = summarize(m["rows"])
    by = {}
    for cls in sorted({r["cls"] for r in m["rows"]}):
        by[cls] = {"planner": summarize([r for r in m["rows"] if r["cls"] == cls]), "prediction_ratio": pred_ratio(recs, cls)}
    scope = {
        "in_auto_select_scope": summarize([r for r in m["rows"] if r["cls"] in validated]),
        "outside_scope_R5d_or_risk_policy": summarize([r for r in m["rows"] if r["cls"] not in validated]),
    }
    worst = sorted(m["rows"], key=lambda r: -r["penalty_ms"])[:8]
    out = {"validated_classes": sorted(validated), "overall": overall, "baselines": baselines(recs), "pg_prediction": pred_ratio(recs), "by_class": by, "scope": scope, "unsafe_selections": m["unsafe"], "correctness_mismatches": m["mismatches"],
           "worst_penalties": [{k: (round(v, 1) if isinstance(v, float) else v) for k, v in r.items()} for r in worst]}
    a.out.write_text(json.dumps(out, indent=2) + "\n", encoding="utf-8", newline="\n")
    print(json.dumps({k: out[k] for k in ("overall", "baselines", "pg_prediction", "unsafe_selections", "correctness_mismatches")}, indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
