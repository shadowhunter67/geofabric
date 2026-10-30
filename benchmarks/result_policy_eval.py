"""Evaluate the fresh result-policy confirmation set against the classes frozen in result_policy_freeze.json.

Final numbers are the REAL planner's recorded decisions (compiled thresholds: duckdb-timeout-risk-v2 0.10, result-policy-v1). The only replay is the 0.20
context column; it is trusted only if the replay reproduces the real planner on every non-refused case at the compiled threshold (parity check).

    python benchmarks/result_policy_eval.py --set raw/rp_heavy.jsonl --out result-policy-eval-rp_heavy.json
"""

from __future__ import annotations

import argparse
import json
import sys
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from metric_safety_eval import T, cands, summary, t_of  # noqa: E402
from postgis_cost_lib import load_raw  # noqa: E402
from risk_threshold_eval import duck_p, pg_ok  # noqa: E402

CAP = 1_000_000
COMPILED = 0.10
BASE = HERE / "results" / "postgis-cost"


def planner_ok(rec: dict) -> bool:
    return bool(rec["planner"].get("chosen"))


def actual_rows(rec: dict) -> int:
    return max((rec[e]["rows"] for e in ("duckdb", "postgis") if rec[e]["status"] == "ok"), default=0)


def fail_kind(rec: dict, e: str) -> str | None:
    """Why engine `e` produced no result: the harness labels every message containing "limit" as `limit_exceeded`, which also covers DuckDB's memory limit."""
    x = rec[e]
    if x["status"] == "ok":
        return None
    if x["status"] == "timeout":
        return "timeout"
    m = (x.get("message") or "").lower()
    if "out of memory" in m:
        return "oom"
    if "row" in m and "limit" in m:
        return "row_cap"
    return x["status"]


def alt_liftable(rec: dict, other: str) -> bool:
    """The other engine could legitimately have been chosen under the policy (feasible, or only deferred by the overridable risk block) and is not itself timeout_risk."""
    if other == "postgis":
        return pg_ok(rec)
    c = cands(rec["planner"]).get("duckdb") or {}
    risk = (c.get("cost_model") or {}).get("risk") or {}
    return bool(c) and c.get("feasible", True) and risk.get("level") != "timeout_risk"


def level(rec: dict, e: str) -> str | None:
    return (((cands(rec["planner"]).get(e) or {}).get("cost_model") or {}).get("risk") or {}).get("level")


def base_choice(rec: dict) -> str:
    real, p = rec["planner"]["chosen"], duck_p(rec)
    if real == "postgis" and p is not None and p >= COMPILED and (cands(rec["planner"]).get("postgis") or {}).get("cost_model", {}).get("validation") != "validated":
        return "duckdb"
    return real


def replay(rec: dict, theta: float) -> str:
    b, p = base_choice(rec), duck_p(rec)
    return "postgis" if b == "duckdb" and p is not None and p >= theta and pg_ok(rec) else b


def classify(rec: dict) -> dict:
    pl = rec["planner"]
    out = {"id": rec["id"], "rows_actual": actual_rows(rec), "p_duck": duck_p(rec) if planner_ok(rec) else None}
    if not planner_ok(rec):
        err = pl.get("error", "")
        out["klass"] = "NO_SAFE_PLAN" if "NO_SAFE_PLAN" in err else "REFUSED_OTHER"
        out["cap_violation"] = "NO_SAFE_PLAN" not in err
        out["refusal_over_conservative"] = 0 < out["rows_actual"] <= CAP
        return out
    c = pl["chosen"]
    o = "postgis" if c == "duckdb" else "duckdb"
    tc, to = t_of(rec, c), t_of(rec, o)
    out.update(chosen=c, verdict=(pl.get("result_delivery") or {}).get("verdict"), noted=bool(pl.get("uncertainty")), level=level(rec, c), tc=tc, to=to)
    out["cap_violation"] = out["rows_actual"] > CAP
    k = "SAFE"
    if tc is not None and tc >= T:
        k = "AVOIDABLE_UNSAFE" if (to is not None and to < T and alt_liftable(rec, o)) else "UNAVOIDABLE_UNCERTAIN"
        out["chosen_timeout"] = True
    elif out["level"] == "timeout_risk":
        if not out["noted"] and alt_liftable(rec, o):
            k = "AVOIDABLE_UNSAFE"
        elif out["noted"]:
            k = "UNAVOIDABLE_UNCERTAIN" if not alt_liftable(rec, o) else "AVOIDED_IN_PRINCIPLE_NOTED"
    out["klass"] = k
    out["chosen_fail"], out["other_fail"] = fail_kind(rec, c), fail_kind(rec, o)
    if tc is not None and to is not None:
        best = min(tc, to)
        out["regret"], out["penalty_ms"] = tc / max(best, 1.0), tc - best
        out["decidable"] = bool(rec.get("identical")) or tc >= T or to >= T
    out["mismatch"] = bool(rec["duckdb"]["status"] == "ok" and rec["postgis"]["status"] == "ok" and not rec.get("identical"))
    return out


def band(x, edges, labels):
    if x is None:
        return "n/a"
    for e, lab in zip(edges, labels):
        if x < e:
            return lab
    return labels[-1]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--set", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    recs = load_raw(BASE / a.set)
    cls = [classify(r) for r in recs]
    by = {r["id"]: r for r in recs}
    n = Counter(c["klass"] for c in cls)
    planned = [c for c in cls if c["klass"] not in ("NO_SAFE_PLAN", "REFUSED_OTHER")]
    dec = [c for c in planned if c.get("decidable") and "regret" in c]
    parity_bad = [(c["id"], c["chosen"], replay(by[c["id"]], COMPILED)) for c in planned if c["chosen"] != replay(by[c["id"]], COMPILED)]
    ctx = {}
    if not parity_bad:
        for th in (0.20, COMPILED):
            rows, unsafe, oom = [], 0, 0
            for c in dec:
                r = by[c["id"]]
                e = replay(r, th)
                t, b = t_of(r, e), min(t_of(r, "duckdb"), t_of(r, "postgis"))
                rows.append({"regret": t / max(b, 1.0), "penalty_ms": t - b})
                unsafe += int(t >= T and b < T)
                oom += int(fail_kind(r, e) == "oom" and fail_kind(r, "postgis" if e == "duckdb" else "duckdb") is None)
            ctx[str(th)] = {**summary(rows), "chosen_timeouts_alt_completed": unsafe, "chosen_oom_alt_completed_in_all_planned": None}
    stats = {
        "set": a.set, "cases": len(cls), "classes": dict(n), "planned": len(planned), "decidable_planned": len(dec),
        "avoidable_unsafe": [c["id"] for c in cls if c["klass"] == "AVOIDABLE_UNSAFE"],
        "unavoidable_uncertain": [c["id"] for c in cls if c["klass"] == "UNAVOIDABLE_UNCERTAIN"],
        "chosen_engine_timeouts": [c["id"] for c in planned if c.get("chosen_timeout")],
        "chosen_timeouts_where_alt_existed": [c["id"] for c in planned if c.get("chosen_timeout") and c["klass"] == "AVOIDABLE_UNSAFE"],
        "result_cap_violations": [c["id"] for c in cls if c["cap_violation"]],
        "correctness_mismatches": [c["id"] for c in planned if c.get("mismatch")],
        "no_safe_plan": [{"id": c["id"], "rows_actual": c["rows_actual"], "over_conservative": c["refusal_over_conservative"]} for c in cls if c["klass"] == "NO_SAFE_PLAN"],
        "chosen_engine_failures_not_timeout": [{"id": c["id"], "kind": c["chosen_fail"], "alt_completed": c["other_fail"] is None, "alt_liftable": alt_liftable(by[c["id"]], "postgis" if c["chosen"] == "duckdb" else "duckdb")} for c in planned if c.get("chosen_fail") and c["chosen_fail"] != "timeout"],
        "failure_kinds_all_runs": dict(Counter(f"{e}:{fail_kind(r, e)}" for r in recs for e in ("duckdb", "postgis") if fail_kind(r, e))),
        "alternate_engine_saved": [c["id"] for c in planned if c["chosen"] == "postgis" and c["tc"] is not None and c["to"] is not None and c["to"] >= T and c["tc"] < T],
        "replay_parity_bad": parity_bad,
        "real_planner": summary(dec),
        "replay_context": ctx,
        "verdicts": dict(Counter(str((c["chosen"], c["verdict"])) for c in planned)),
    }
    for fn, store in ((lambda c: band(c["p_duck"], [0.08, 0.12, 0.18, 0.22, 9], ["<0.08", "0.08-0.12", "0.12-0.18", "0.18-0.22", ">=0.22"]), "by_p_band"),
                      (lambda c: band(c["rows_actual"], [5e5, 8e5, 9.5e5, 1.0e6, 1.05e6, 1.2e6, 2e6, 1e18], ["<500k", "500-800k", "800-950k", "950k-1M", "1-1.05M", "1.05-1.2M", "1.2-2M", ">=2M"]), "by_rows_band")):
        g: dict = {}
        for c in cls:
            g.setdefault(fn(c), []).append(c)
        stats[store] = {k: {"n": len(v), "classes": dict(Counter(x["klass"] for x in v))} for k, v in sorted(g.items())}
    (BASE / a.out).write_text(json.dumps(stats, indent=1, ensure_ascii=False), encoding="utf-8")
    print(json.dumps(stats, indent=1, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
