"""Offline error decomposition + PostGIS metric_small time-model variants (discovery fit -> calibration evaluation). Benchmark-time only."""
import json, sys
from pathlib import Path
import numpy as np
sys.path.insert(0, str(Path(__file__).resolve().parent))
from postgis_cost_lib import load_raw
R = Path(__file__).resolve().parent / "results" / "postgis-cost"
TR = json.loads((R / "truth-ms.json").read_text())

def load(split):
    out = []
    for r in load_raw(R / "raw" / f"{split}.jsonl"):
        sj = r["planner"]["workload"].get("semi_join")
        k = f"{split}:{r['id']}"
        if not sj or k not in TR or r["postgis"]["status"] != "ok" or r["duckdb"]["status"] != "ok" or not r.get("identical", True):
            continue
        if r["planner"]["workload"].get("semi_join", {}).get("expected_probes", 1e9) > 5000 * 1.0 and False:
            continue
        t = TR[k]
        out.append(dict(id=r["id"], pg=r["postgis"]["warm_median_ms"], dk=r["duckdb"]["warm_median_ms"], probes=sj["expected_probes"], right=sj["right_rows_filtered"],
                        pairs=sj["candidate_pairs"], d=sj["distance_m"], cpp=sj["candidates_per_probe"], ovl=sj["overlap_ratio"], **{"t_" + a: b for a, b in t.items()}))
    return out

def err(est, tru):  # symmetric ratio, floor 1
    e, t = np.maximum(est, 1.0), np.maximum(tru, 1.0)
    return np.maximum(e / t, t / e)

def q(x): return f"med x{np.median(x):.2f} P90 x{np.percentile(x,90):.2f}"

def X(rows, names):
    cols = {"ln_probes": lambda r: np.log(max(r["probes"], 1)), "ln_right": lambda r: np.log(max(r["right"], 1)), "ln_pairs": lambda r: np.log(max(r["pairs"], 1)),
            "ln_d": lambda r: np.log(r["d"]), "ln_tleft": lambda r: np.log(max(r["t_left"], 1)), "ln_tright": lambda r: np.log(max(r["t_right"], 1)),
            "ln_tpairs": lambda r: np.log(max(r["t_pairs"], 1)), "ovl": lambda r: r["ovl"]}
    return np.array([[1.0] + [cols[n](r) for n in names] for r in rows])

def fit(rows, names):
    y = np.log([max(r["pg"], 1.0) for r in rows]); A = X(rows, names)
    return np.linalg.lstsq(A, y, rcond=None)[0]

def pred_ratio(rows, names, w):
    p = np.exp(X(rows, names) @ w); return err(p, np.array([r["pg"] for r in rows])) if False else np.maximum(np.maximum(p, 10) / np.maximum([r["pg"] for r in rows], 10), np.maximum([r["pg"] for r in rows], 10) / np.maximum(p, 10))

if __name__ == "__main__":
    D, C = load("ms_disc"), load("ms_cal")
    print("usable disc", len(D), "cal", len(C))
    for nm, rows in (("disc", D), ("cal", C)):
        a = lambda k: np.array([r[k] for r in rows], float)
        print(nm, "left est vs true", q(err(a("probes"), a("t_left"))), "| right(filtered)", q(err(a("right"), a("t_right"))), "| pairs", q(err(a("pairs"), a("t_pairs"))))
        print("   bias (est/true median): left", np.median(a("probes") / np.maximum(a("t_left"), 1)).round(2), "right", np.median(a("right") / np.maximum(a("t_right"), 1)).round(2), "pairs", np.median(a("pairs") / np.maximum(a("t_pairs"), 1)).round(2))
    sets = {"current [probes,right]": ["ln_probes", "ln_right"], "+ln_pairs(est)": ["ln_probes", "ln_right", "ln_pairs"], "pairs(est) only": ["ln_pairs"],
            "+ln_d": ["ln_probes", "ln_right", "ln_d"], "probes,right,pairs,d": ["ln_probes", "ln_right", "ln_pairs", "ln_d"],
            "ORACLE true left,right": ["ln_tleft", "ln_tright"], "ORACLE true pairs": ["ln_tpairs"], "ORACLE left,pairs": ["ln_tleft", "ln_tpairs"], "ORACLE left,right,pairs,d": ["ln_tleft", "ln_tright", "ln_tpairs", "ln_d"]}
    print("\nPostGIS time model: fit on DISCOVERY, evaluated on CALIBRATION (ratio with 10 ms floor)")
    for k, names in sets.items():
        w = fit(D, names); rd, rc = pred_ratio(D, names, w), pred_ratio(C, names, w)
        print(f"  {k:28s} in-sample {q(rd)} | held-out {q(rc)}  coef {np.round(w,2).tolist()}")
