"""Compare the frozen candidate DuckDB timeout_risk thresholds on a measured set.

The measured records carry, for every case, both engines' outcomes (timeouts censored at 60 s) and what the REAL planner decided with the thresholds that
were compiled in (0.20). A candidate threshold only changes ONE thing in the planner: the P(timeout) at which DuckDB counts as `timeout_risk`. So for every
other threshold the decision is re-derived from the same recorded candidates with the same rule R5e applies:

    base choice (before R5e) = DuckDB in the limited metric classes (R5d), PostGIS when its class is validated
    if the base is DuckDB and its P(timeout) >= threshold and PostGIS is feasible and not itself timeout_risk  ->  PostGIS

The re-derivation is checked against the real planner at 0.20 on every record (`replay_matches_real_planner_at_0.20` must be true, otherwise the
comparison is not trusted). After a threshold is selected and written into the model file the real planner is re-run on the set as the final check.

    python benchmarks/risk_threshold_eval.py --set raw/thr_heavy.jsonl [--context raw/msj_heavy.jsonl ...] --out eval.json
"""

from __future__ import annotations

import argparse
import json
import statistics
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from metric_safety_eval import T, cands, decidable, summary, t_of  # noqa: E402
from postgis_cost_lib import load_raw  # noqa: E402

FROZEN = json.loads((Path(__file__).resolve().parent / "risk_threshold_candidates.json").read_text(encoding="utf-8"))
THETAS = FROZEN["candidate_timeout_risk_thresholds"]
CURRENT = FROZEN["current"]
MAT_RATIO = 2.0
MAT_MS = 1000.0


def duck_p(rec: dict) -> float | None:
    cm = (cands(rec["planner"]).get("duckdb") or {}).get("cost_model") or {}
    return (cm.get("risk") or {}).get("timeout_probability")


def pg_ok(rec: dict) -> bool:
    """PostGIS is a valid R5e target iff it is feasible OR only deferred by its overridable cost-based risk block (R5e lifts that block, lib.rs
    `safe_deferred`), and its own verdict is not `timeout_risk`."""
    c = cands(rec["planner"]).get("postgis") or {}
    risk = (c.get("cost_model") or {}).get("risk") or {}
    # deferred = blocked ONLY by the overridable cost-based risk policy; any other reason (result row cap, unsupported op, residency) stays infeasible
    deferred = bool(risk.get("blocked")) and bool(risk.get("overridable")) and all(str(x).startswith("risk policy") for x in c.get("rejected_because") or [])
    return bool(c) and (c.get("feasible", True) or deferred) and risk.get("level") != "timeout_risk"


def base_choice(rec: dict) -> str:
    """The engine R5d alone would choose. The real choice is that unless R5e moved it: DuckDB -> PostGIS happens only when P >= CURRENT."""
    real = rec["planner"].get("chosen")
    p = duck_p(rec)
    if real == "postgis" and p is not None and p >= CURRENT and (cands(rec["planner"]).get("postgis") or {}).get("cost_model", {}).get("validation") != "validated":
        return "duckdb"
    return real


def decide(rec: dict, theta: float) -> str:
    b = base_choice(rec)
    p = duck_p(rec)
    return "postgis" if b == "duckdb" and p is not None and p >= theta and pg_ok(rec) else b


def evaluate(recs: list[dict], theta: float) -> dict:
    dec = [r for r in recs if decidable(r)]
    rows, unsafe, reroutes, unnecessary, saved = [], [], 0, [], []
    for r in dec:
        a, b = t_of(r, "duckdb"), t_of(r, "postgis")
        eng = decide(r, theta)
        sel, other = (a, b) if eng == "duckdb" else (b, a)
        best = min(a, b)
        rows.append({"id": r["id"], "engine": eng, "regret": sel / max(best, 1.0), "penalty_ms": sel - best, "timeout": sel >= T})
        if sel >= T and other < T:
            unsafe.append(r["id"])
        base = base_choice(r)
        if eng != base:
            reroutes += 1
            if a < T and b >= MAT_RATIO * a and b - a >= MAT_MS:
                unnecessary.append(r["id"])
        if base == "duckdb" and a >= T and eng == "postgis" and b < T:
            saved.append(r["id"])
    s = summary(rows)
    return {"threshold": theta, "decidable": len(dec), "unsafe": len(unsafe), "unsafe_ids": unsafe, "chosen_engine_timeouts": sum(1 for x in rows if x["timeout"]),
            "reroutes": reroutes, "unnecessary_reroutes": len(unnecessary), "unnecessary_ids": unnecessary, "alternate_engine_saved": len(saved),
            "p50_regret": s.get("p50_regret"), "p90_regret": s.get("p90_regret"), "p95_regret": s.get("p95_regret"), "max_regret": s.get("max_regret"),
            "mean_penalty_ms": s.get("mean_penalty_ms"), "max_penalty_ms": s.get("max_penalty_ms")}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--set", type=Path, required=True)
    ap.add_argument("--context", type=Path, nargs="*", default=[])
    ap.add_argument("--out", type=Path, required=True)
    a = ap.parse_args()
    out: dict = {}
    for label, paths in (("fresh", [a.set]), ("context", a.context)):
        if not paths:
            continue
        recs = [r for p in paths for r in load_raw(p)]
        dec = [r for r in recs if decidable(r)]
        match = all(decide(r, CURRENT) == r["planner"].get("chosen") for r in dec)
        out[label] = {"cases": len(recs), "decidable": len(dec), "replay_matches_real_planner_at_0.20": match,
                      "mismatches": [r["id"] for r in dec if decide(r, CURRENT) != r["planner"].get("chosen")][:10],
                      "failures": {e: {s: sum(1 for r in recs if r[e]["status"] == s) for s in ("ok", "timeout", "error")} for e in ("duckdb", "postgis")},
                      "correctness_mismatches": sum(1 for r in recs if r.get("comparable") and not r.get("identical")),
                      "duckdb_timeouts": sum(1 for r in dec if t_of(r, "duckdb") >= T), "thresholds": [evaluate(recs, t) for t in THETAS],
                      "always_duckdb": evaluate([dict(r, planner=dict(r["planner"], chosen="duckdb")) for r in recs], 2.0),
                      "duckdb_p_of_every_timeout": sorted(round(duck_p(r) or -1, 3) for r in dec if t_of(r, "duckdb") >= T)}
    a.out.write_text(json.dumps(out, indent=1) + "\n", encoding="utf-8", newline="\n")
    for label, v in out.items():
        print(f"== {label}: {v['cases']} cases, {v['decidable']} decidable, DuckDB timeouts {v['duckdb_timeouts']}, replay==planner@0.20: {v['replay_matches_real_planner_at_0.20']} {v['mismatches']}")
        print(f"   {'theta':>6} {'unsafe':>6} {'chosenTO':>8} {'reroute':>7} {'unneeded':>8} {'saved':>5} {'P50':>5} {'P90':>6} {'P95':>6} {'mean pen ms':>11} {'max pen ms':>10}")
        for t in v["thresholds"]:
            print(f"   {t['threshold']:>6} {t['unsafe']:>6} {t['chosen_engine_timeouts']:>8} {t['reroutes']:>7} {t['unnecessary_reroutes']:>8} {t['alternate_engine_saved']:>5} {t['p50_regret']:>5} {t['p90_regret']:>6} {t['p95_regret']:>6} {t['mean_penalty_ms']:>11} {t['max_penalty_ms']:>10}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
